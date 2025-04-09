use crate::{
    config::AppConfig,
    error::AppError,
    services::{proxy, transaction},
    types::{rpc::RpcRequest, whitelist},
};
use axum::{
    extract::{Json, State},
    response::IntoResponse,
};
use tracing::{debug, warn};

/// Handles JSON-RPC requests and routes them to the appropriate handler.
pub async fn handle_rpc(
    State(config): State<AppConfig>,
    Json(req): Json<RpcRequest>,
) -> impl IntoResponse {
    // Check if the whitelist is enabled in configuration
    if config.security.enable_whitelist && !whitelist::is_method_allowed(&req.method) {
        warn!("Unauthorized RPC method call attempted: {}", req.method);
        return Json(AppError::MethodNotAllowed(req.method.clone()).to_json_rpc_error(req.id));
    }

    debug!("Processing RPC method: {}", req.method);

    // Method is allowed or whitelist is disabled, proceed with normal processing
    match req.method.as_str() {
        "eth_sendRawTransaction" => transaction::handle_send_raw_transaction(req, &config).await,
        _ => proxy::forward_to_el(req, &config.el.url).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ElConfig, SecurityConfig, ServerConfig, WalletConfig};
    use axum::{body::to_bytes, extract::State};
    use serde_json::{json, Value};

    // Helper to create a default test config with a fake EL URL
    fn test_config(enable_whitelist: bool) -> AppConfig {
        AppConfig {
            server: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 8535,
            },
            el: ElConfig {
                // In tests, we'll use a fake URL
                url: "http://localhost:12345".to_string(),
            },
            wallet: WalletConfig {
                wallet_daemon_uri: "http://localhost:8082".to_string(),
                to_address: "".to_string(),
            },
            security: SecurityConfig { enable_whitelist },
        }
    }

    // Helper to create RPC request with arbitrary method
    fn create_rpc_request(method: &str) -> RpcRequest {
        RpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params: json!([]),
            id: json!(1),
        }
    }

    #[tokio::test]
    async fn test_blocked_method_rejected_by_whitelist() {
        // Arrange - No mock needed as request should be rejected before reaching EL
        let config = test_config(true); // whitelist enabled
        let request = create_rpc_request("debug_traceTransaction"); // non-whitelisted method

        // Act - Execute the handler
        let response = handle_rpc(State(config), Json(request)).await;

        // Convert the response to bytes
        let response = response.into_response();
        let body_bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let response_json: Value = serde_json::from_slice(&body_bytes).unwrap();

        // Assert
        assert!(
            response_json.get("error").is_some(),
            "Response should contain an error"
        );
        assert_eq!(
            response_json["error"]["code"],
            json!(-32002),
            "Error code should be -32002"
        );
        assert!(
            response_json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("debug_traceTransaction"),
            "Error message should mention the method"
        );
    }
}
