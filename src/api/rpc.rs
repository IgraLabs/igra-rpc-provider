use crate::{
    error::AppError,
    services::transaction,
    types::{rpc::RpcRequest, whitelist},
    AppState,
};
use axum::{
    extract::{Json, State},
    response::IntoResponse,
};
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use tracing::{error, info, warn};

/// Handles JSON-RPC requests and routes them to the appropriate handler.
/// Focuses solely on HTTP request/response handling and delegates business logic to services.
pub async fn handle_rpc(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RpcRequest>,
) -> impl IntoResponse {
    let request_context = RequestContext::new(&req);

    // Log incoming request
    log_incoming_request(&request_context);

    // Validate request authorization
    if let Some(error_response) = validate_request_authorization(&state, &req, &request_context) {
        return error_response;
    }

    // Route request to appropriate service and measure performance
    let start_time = Instant::now();
    let result = route_request_to_service(&state, req, &request_context).await;
    let duration = start_time.elapsed();

    // Log response and return
    log_response(&request_context, &result, duration);
    Json(result)
}

/// Context information for request processing and logging
struct RequestContext {
    method: String,
    id: String,
    payload_info: PayloadInfo,
}

/// Information about request payload for logging
struct PayloadInfo {
    params_summary: String,
    estimated_size: usize,
}

impl RequestContext {
    fn new(req: &RpcRequest) -> Self {
        let method = req.method.clone();
        let id = req.id.to_string();
        let payload_info = PayloadInfo::extract_from_request(req);

        Self {
            method,
            id,
            payload_info,
        }
    }
}

impl PayloadInfo {
    fn extract_from_request(req: &RpcRequest) -> Self {
        let params_summary = match req.params.get(0) {
            Some(param) => param.to_string(),
            None => "empty".to_string(),
        };

        let estimated_size = req
            .params
            .get(0)
            .and_then(|v| v.as_str())
            .map(|s| (s.len() / 2).saturating_sub(1)) // Rough estimate: hex string / 2 - 1 for 0x
            .unwrap_or(0);

        Self {
            params_summary,
            estimated_size,
        }
    }
}

/// Log incoming request with appropriate detail level
fn log_incoming_request(ctx: &RequestContext) {
    info!(
        "RPC REQUEST [id={}]: Received method={}",
        ctx.id, ctx.method
    );

    if ctx.method == "eth_sendRawTransaction" {
        info!(
            "RPC REQUEST [id={}]: Processing transaction, params={}, est_payload_size={} bytes",
            ctx.id, ctx.payload_info.params_summary, ctx.payload_info.estimated_size
        );
    }
}

/// Validate request authorization using whitelist if enabled
fn validate_request_authorization(
    state: &Arc<AppState>,
    req: &RpcRequest,
    ctx: &RequestContext,
) -> Option<Json<Value>> {
    if state.config.security.enable_whitelist && !whitelist::is_method_allowed(&ctx.method) {
        warn!("Unauthorized RPC method call attempted: {}", ctx.method);
        return Some(Json(
            AppError::MethodNotAllowed(ctx.method.clone()).to_json_rpc_error(req.id.clone()),
        ));
    }
    None
}

/// Route request to the appropriate service based on method
async fn route_request_to_service(
    state: &Arc<AppState>,
    req: RpcRequest,
    ctx: &RequestContext,
) -> Value {
    match ctx.method.as_str() {
        "eth_sendRawTransaction" => {
            info!(
                "RPC REQUEST [id={}]: Routing to transaction service",
                ctx.id
            );
            handle_transaction_request(state, req).await
        }
        _ => {
            info!(
                "RPC REQUEST [id={}]: Routing to proxy service for EL forwarding",
                ctx.id
            );
            handle_proxy_request(state, req).await
        }
    }
}

/// Handle transaction processing requests
async fn handle_transaction_request(state: &Arc<AppState>, req: RpcRequest) -> Value {
    transaction::process_transaction(req, state.clone()).await
}

/// Handle proxy forwarding requests
async fn handle_proxy_request(state: &Arc<AppState>, req: RpcRequest) -> Value {
    let result = state.proxy_service.forward_to_el(req).await;
    result.0 // Extract the Value from Json wrapper
}

/// Log response with appropriate detail level
fn log_response(ctx: &RequestContext, result: &Value, duration: std::time::Duration) {
    if let Some(result_value) = result.get("result") {
        if ctx.method == "eth_sendRawTransaction" {
            let tx_hash = result_value.as_str().unwrap_or("unknown");
            info!(
                "RPC RESPONSE [id={}, hash={}]: Transaction processed successfully, time={:?}, payload_size={} bytes",
                ctx.id, tx_hash, duration, ctx.payload_info.estimated_size
            );
        } else {
            info!(
                "RPC RESPONSE [id={}]: Request completed successfully, time={:?}",
                ctx.id, duration
            );
        }
    } else if let Some(error) = result.get("error") {
        if ctx.method == "eth_sendRawTransaction" {
            error!(
                "RPC RESPONSE [id={}]: Transaction processing failed, error={}, time={:?}, payload={}",
                ctx.id, error, duration, ctx.payload_info.params_summary
            );
        } else {
            error!(
                "RPC RESPONSE [id={}]: Request failed, error={:?}, time={:?}",
                ctx.id, error, duration
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AppConfig, GasConfig, MiningConfig, ProxyConfig, RetryConfig, SecurityConfig, ServerConfig,
        WalletConfig,
    };
    use serde_json::json;

    // Helper to create a default test config with a fake EL URL
    fn test_config(enable_whitelist: bool) -> AppConfig {
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
            security: SecurityConfig::with_whitelist(enable_whitelist),
            mining: MiningConfig::default(),
            gas: GasConfig::with_min_protocol_fee_per_gas_gwei(100),
            retry: RetryConfig::default(),
        }
    }

    // Direct test for whitelist validation without involving the full handler
    #[test]
    fn test_method_allowed_by_whitelist() {
        assert!(whitelist::is_method_allowed("eth_getBalance"));
        assert!(whitelist::is_method_allowed("debug_traceTransaction"));
        assert!(!whitelist::is_method_allowed("admin_addPeer")); // Example of a method not in whitelist
    }

    // Test the error response format for disallowed methods
    #[test]
    fn test_error_format_for_disallowed_method() {
        let method = "admin_addPeer".to_string();
        let id = json!(1);
        let error_json = AppError::MethodNotAllowed(method.clone()).to_json_rpc_error(id);

        // Access the JSON fields directly
        assert_eq!(error_json["error"]["code"], json!(-32002));
        assert!(error_json["error"]["message"]
            .as_str()
            .expect("Error message should be a string")
            .contains(&method));
    }

    // Test the whitelist check in the config
    #[test]
    fn test_whitelist_check_in_config() {
        let config = test_config(true); // whitelist enabled
        assert!(config.security.enable_whitelist);

        let config2 = test_config(false); // whitelist disabled
        assert!(!config2.security.enable_whitelist);
    }
}
