use crate::{api::routing, types::rpc::RpcEnvelope, AppState};
use axum::{body::Bytes, extract::State, response::IntoResponse, Json};
use serde_json::Value;
use std::sync::Arc;
use tracing::warn;

/// Handles JSON-RPC requests and routes them to the appropriate handler.
/// Supports both single and batch requests, delegating business logic to services.
///
/// The raw body is parsed manually (rather than via axum's `Json` extractor) so that malformed
/// input returns a proper JSON-RPC error object instead of axum's default plain-text `422`
/// rejection. Same strategy as the WebSocket handler (a JSON-RPC error rather than a transport-level
/// rejection), but it classifies the failure more precisely since the body can be re-parsed:
/// `-32700` for invalid JSON vs `-32600` for a valid-JSON request of the wrong shape.
pub async fn handle_rpc(State(state): State<Arc<AppState>>, body: Bytes) -> impl IntoResponse {
    let envelope = match parse_rpc_envelope(&body) {
        Ok(envelope) => envelope,
        Err(error_response) => return Json(error_response),
    };

    match envelope {
        RpcEnvelope::Single(req) => Json(routing::route_and_process(&state, req).await),
        RpcEnvelope::Batch(mut requests) => {
            if requests.is_empty() {
                // JSON-RPC 2.0: empty batch is an invalid request; return single error object
                return Json(routing::json_rpc_error(
                    Value::Null,
                    -32600,
                    "Invalid Request: empty batch",
                ));
            }

            let mut responses = Vec::with_capacity(requests.len());
            for req in requests.drain(..) {
                let value = routing::route_and_process(&state, req).await;
                responses.push(value);
            }

            Json(Value::Array(responses))
        }
    }
}

/// Parse a raw HTTP body into an `RpcEnvelope`, returning a ready-made JSON-RPC error object on
/// failure. Distinguishes `-32700` (the body is not valid JSON) from `-32600` (valid JSON that is
/// not a valid JSON-RPC request/batch, e.g. a request missing the `jsonrpc` field), recovering the
/// request `id` when present. The underlying parser detail is logged server-side rather than
/// returned to the client, so the response carries only a stable code/message — consistent with
/// `routing::json_rpc_error` and the rest of the codebase's JSON-RPC errors.
fn parse_rpc_envelope(body: &[u8]) -> Result<RpcEnvelope, Value> {
    match serde_json::from_slice::<RpcEnvelope>(body) {
        Ok(envelope) => Ok(envelope),
        // The body did not match the envelope; re-parse as a generic value to classify the failure.
        Err(envelope_err) => match serde_json::from_slice::<Value>(body) {
            // Valid JSON, but not a valid Request object/batch -> Invalid Request (echo id if present).
            Ok(value) => {
                warn!("Rejecting invalid JSON-RPC request: {envelope_err}");
                let id = value.get("id").cloned().unwrap_or(Value::Null);
                Err(routing::json_rpc_error(id, -32600, "Invalid Request"))
            }
            // Not valid JSON at all -> Parse error.
            Err(parse_err) => {
                warn!("Rejecting unparsable request body: {parse_err}");
                Err(routing::json_rpc_error(Value::Null, -32700, "Parse error"))
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use crate::api::routing::{json_rpc_error, PayloadInfo, RequestContext};
    use crate::config::{
        AppConfig, GasConfig, LaneConfig, MiningConfig, ProxyConfig, RetryConfig, SecurityConfig,
        ServerConfig, WalletConfig,
    };
    use crate::error::AppError;
    use crate::types::rpc::RpcRequest;
    use crate::types::whitelist;
    use serde_json::{json, Value};

    // Test configuration builder for cleaner test setup
    struct TestConfigBuilder {
        enable_whitelist: bool,
        read_only: bool,
    }

    impl TestConfigBuilder {
        fn new() -> Self {
            Self {
                enable_whitelist: false,
                read_only: false,
            }
        }

        fn with_whitelist(mut self, enable: bool) -> Self {
            self.enable_whitelist = enable;
            self
        }

        fn with_read_only(mut self, enable: bool) -> Self {
            self.read_only = enable;
            self
        }

        fn build(self) -> AppConfig {
            AppConfig {
                server: ServerConfig {
                    host: "127.0.0.1".to_string(),
                    port: 8535,
                },
                proxy: ProxyConfig::with_el_url("http://localhost:12345".to_string()),
                wallet: WalletConfig {
                    wallet_daemon_uri: "http://localhost:8082".to_string(),
                    to_address: "".to_string(),
                },
                security: SecurityConfig {
                    enable_whitelist: self.enable_whitelist,
                    read_only: self.read_only,
                },
                mining: MiningConfig::default(),
                // Tests intentionally run without KIP-21 lane enforcement
                // (no real wallet daemon). Use the explicit escape hatch
                // so the production-safe default (require IGRA_LANE_ID)
                // can stay required-by-default.
                lane: LaneConfig::disabled(),
                gas: GasConfig::default(),
                retry: RetryConfig::default(),
            }
        }
    }

    // Helper to create a default test config with a fake EL URL
    fn test_config(enable_whitelist: bool) -> AppConfig {
        TestConfigBuilder::new()
            .with_whitelist(enable_whitelist)
            .build()
    }

    // Direct test for whitelist validation without involving the full handler
    #[test]
    fn test_method_allowed_by_whitelist() {
        assert!(whitelist::is_method_allowed("eth_getBalance"));
        assert!(whitelist::is_method_allowed("debug_traceTransaction"));
        assert!(!whitelist::is_method_allowed("admin_addPeer")); // Example of a method not in whitelist
    }

    fn assert_error_response(
        error_json: &Value,
        expected_code: i32,
        expected_message_contains: Option<&str>,
    ) {
        assert_eq!(error_json["error"]["code"], json!(expected_code));

        if let Some(expected_text) = expected_message_contains {
            let message = error_json["error"]["message"]
                .as_str()
                .expect("Error message should be a string");
            assert!(
                message.contains(expected_text),
                "Expected message to contain '{expected_text}', but got: '{message}'"
            );
        }
    }

    // Test the error response format for disallowed methods
    #[test]
    fn test_error_format_for_disallowed_method() {
        let method = "admin_addPeer".to_string();
        let id = json!(1);
        let error_json = AppError::MethodNotAllowed(method.clone()).to_json_rpc_error(id);

        assert_error_response(&error_json, -32002, Some(&method));
    }

    // Test the whitelist check in the config
    #[test]
    fn test_whitelist_check_in_config() {
        let config_with_whitelist = test_config(true);
        let config_without_whitelist = test_config(false);

        assert!(config_with_whitelist.security.enable_whitelist);
        assert!(!config_without_whitelist.security.enable_whitelist);
    }

    // Helper to create a test config with read-only mode
    fn test_config_read_only(enable_whitelist: bool, read_only: bool) -> AppConfig {
        TestConfigBuilder::new()
            .with_whitelist(enable_whitelist)
            .with_read_only(read_only)
            .build()
    }

    // Test that write methods are blocked in read-only mode
    #[test]
    fn test_read_only_mode_blocks_write_methods() {
        assert!(whitelist::is_write_method("eth_sendRawTransaction"));
        assert!(whitelist::is_write_method("personal_sign"));
        assert!(whitelist::is_write_method("admin_addPeer"));

        assert!(!whitelist::is_write_method("eth_getBalance"));
        assert!(!whitelist::is_write_method("eth_call"));
    }

    // Test the error response format for read-only mode
    #[test]
    fn test_error_format_for_read_only_mode() {
        let id = json!(1);
        let error_json = AppError::ReadOnlyMode.to_json_rpc_error(id);

        assert_error_response(&error_json, -32000, Some("Read-only mode is enabled"));
    }

    // Test the read-only mode configuration
    #[test]
    fn test_read_only_mode_in_config() {
        let config_read_only = test_config_read_only(true, true);
        let config_read_write = test_config_read_only(true, false);

        assert!(config_read_only.security.is_read_only());
        assert!(!config_read_write.security.is_read_only());
    }

    // Test helper for creating RpcRequest instances
    struct TestRpcRequest {
        method: String,
        params: Value,
        id: Value,
    }

    impl TestRpcRequest {
        fn new(method: &str, id: i32) -> Self {
            Self {
                method: method.to_string(),
                params: Value::Array(vec![]),
                id: json!(id),
            }
        }

        fn with_params(mut self, params: Value) -> Self {
            self.params = params;
            self
        }

        fn build(self) -> RpcRequest {
            RpcRequest {
                jsonrpc: "2.0".to_string(),
                method: self.method,
                params: self.params,
                id: self.id,
            }
        }
    }

    fn assert_context_basics(context: &RequestContext, expected_method: &str) {
        assert_eq!(context.method, expected_method);
    }

    fn assert_empty_payload_info(context: &RequestContext) {
        assert_eq!(context.payload_info.params_summary, "empty");
        assert_eq!(context.payload_info.estimated_size, 0);
    }

    // Additional tests for optional params handling
    #[test]
    fn test_request_context_with_missing_params() {
        let request = TestRpcRequest::new("eth_gasPrice", 1).build();
        let context = RequestContext::new(&request);

        assert_context_basics(&context, "eth_gasPrice");
        assert_empty_payload_info(&context);
    }

    #[test]
    fn test_request_context_with_populated_params() {
        let request = TestRpcRequest::new("eth_getBalance", 1)
            .with_params(json!([
                "0x407d73d8a49eeb85d32cf465507dd71d507100c1",
                "latest"
            ]))
            .build();
        let context = RequestContext::new(&request);

        assert_context_basics(&context, "eth_getBalance");
        assert_eq!(
            context.payload_info.params_summary,
            "\"0x407d73d8a49eeb85d32cf465507dd71d507100c1\""
        );
    }

    #[test]
    fn test_request_context_with_null_params() {
        let request = TestRpcRequest::new("eth_chainId", 1)
            .with_params(Value::Null)
            .build();
        let context = RequestContext::new(&request);

        assert_context_basics(&context, "eth_chainId");
        assert_empty_payload_info(&context);
    }

    #[test]
    fn test_payload_info_extract_with_various_param_types() {
        // Test scenarios with different parameter types
        let test_cases = vec![
            (
                "eth_sendRawTransaction",
                json!(["0x1234567890abcdef"]),
                8, // (18/2) - 1 = 8
                "Hex string parameter",
            ),
            (
                "eth_getBalance",
                json!(["0x407d73d8a49eeb85d32cf465507dd71d507100c1", "latest"]),
                20, // (42/2) - 1 = 20
                "Address parameter",
            ),
            (
                "eth_gasPrice",
                Value::Array(vec![]),
                0,
                "Empty params array",
            ),
        ];

        for (method, params, expected_size, description) in test_cases {
            let request = TestRpcRequest::new(method, 1).with_params(params).build();
            let payload_info = PayloadInfo::extract_from_request(&request);

            assert_eq!(
                payload_info.estimated_size, expected_size,
                "Failed for case: {description}"
            );

            if expected_size == 0 {
                assert_eq!(payload_info.params_summary, "empty");
            }
        }
    }

    #[test]
    fn test_json_rpc_error_format() {
        let error_response = json_rpc_error(json!(1), -32600, "Invalid Request");

        assert_eq!(error_response["jsonrpc"], "2.0");
        assert_eq!(error_response["id"], json!(1));
        assert_eq!(error_response["error"]["code"], -32600);
        assert_eq!(error_response["error"]["message"], "Invalid Request");
    }
}

#[cfg(test)]
mod parse_envelope_tests {
    use super::{parse_rpc_envelope, RpcEnvelope};
    use serde_json::Value;

    #[test]
    fn valid_single_request_parses() {
        let body = br#"{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}"#;
        match parse_rpc_envelope(body) {
            Ok(RpcEnvelope::Single(req)) => assert_eq!(req.method, "eth_chainId"),
            other => panic!("expected Single, got {other:?}"),
        }
    }

    #[test]
    fn valid_batch_parses() {
        let body = br#"[{"jsonrpc":"2.0","method":"eth_chainId","id":1}]"#;
        match parse_rpc_envelope(body) {
            Ok(RpcEnvelope::Batch(reqs)) => assert_eq!(reqs.len(), 1),
            other => panic!("expected Batch, got {other:?}"),
        }
    }

    #[test]
    fn missing_jsonrpc_is_invalid_request_with_recovered_id() {
        // The reporter's exact case: valid JSON, missing `jsonrpc` -> -32600, echo id.
        let body = br#"{"id":7,"method":"eth_chainId","params":[]}"#;
        let err = parse_rpc_envelope(body).expect_err("missing jsonrpc should be an error");
        assert_eq!(err["jsonrpc"], "2.0");
        assert_eq!(err["error"]["code"], -32600);
        assert_eq!(err["error"]["message"], "Invalid Request");
        assert_eq!(err["id"], 7);
    }

    #[test]
    fn invalid_json_is_parse_error_with_null_id() {
        let body = b"not json at all";
        let err = parse_rpc_envelope(body).expect_err("invalid json should be an error");
        assert_eq!(err["error"]["code"], -32700);
        assert_eq!(err["error"]["message"], "Parse error");
        assert_eq!(err["id"], Value::Null);
    }

    #[test]
    fn empty_body_is_parse_error() {
        let err = parse_rpc_envelope(b"").expect_err("empty body should be an error");
        assert_eq!(err["error"]["code"], -32700);
    }
}
