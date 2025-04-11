use crate::{
    AppState,
    error::AppError,
    services::{proxy, transaction},
    types::{rpc::RpcRequest, whitelist},
};
use axum::{
    extract::{Json, State},
    response::IntoResponse,
};
use std::sync::Arc;
use tracing::{info, error, warn, debug};

/// Handles JSON-RPC requests and routes them to the appropriate handler.
pub async fn handle_rpc(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RpcRequest>,
) -> impl IntoResponse {
    let method = req.method.clone();
    let id = req.id.to_string();

    // Check if the whitelist is enabled in configuration
    if state.config.security.enable_whitelist && !whitelist::is_method_allowed(&method) {
        warn!("Unauthorized RPC method call attempted: {}", method);
        return Json(AppError::MethodNotAllowed(method.clone()).to_json_rpc_error(req.id));
    }

    info!("RPC REQUEST [id={}]: Received method={}", id, method);

    let result = match method.as_str() {
        "eth_sendRawTransaction" => {
            // Get full params
            let full_params = match req.params.get(0) {
                Some(param) => param.to_string(),
                None => "empty".to_string()
            };

            // Get payload size if possible
            let payload_size = req.params.get(0)
                .and_then(|v| v.as_str())
                .map(|s| s.len() / 2 - 1) // Rough estimate: hex string / 2 - 1 for 0x
                .unwrap_or(0);

            info!("RPC REQUEST [id={}]: Processing transaction, params={}, est_payload_size={} bytes",
                id, full_params, payload_size);

            // Process transaction and return hash immediately, with background wallet processing
            let start_time = std::time::Instant::now();
            let result = transaction::process_transaction(req, state.clone()).await;
            let duration = start_time.elapsed();

            // Extract result or error for logging
            if let Some(result_value) = result.get("result") {
                let tx_hash = result_value.as_str().unwrap_or("unknown");
                info!("RPC RESPONSE [id={}, hash={}]: Transaction processed successfully, time={:?}, payload_size={} bytes",
                    id, tx_hash, duration, payload_size);
            } else if let Some(error) = result.get("error") {
                error!("RPC RESPONSE [id={}]: Transaction processing failed, error={}, time={:?}, payload={}",
                    id, error, duration, full_params);
            }

            axum::Json(result)
        }
        // For all other methods, just forward to EL using the original logic
        _ => {
            info!("RPC REQUEST [id={}]: Forwarding method={} to execution layer at {}",
                id, method, state.config.el.url);

            let start_time = std::time::Instant::now();
            let result = proxy::forward_to_el(req, &state.config.el.url).await;
            let duration = start_time.elapsed();

            // Extract result or error for logging
            if result.0.get("error").is_some() {
                error!("RPC RESPONSE [id={}]: Execution layer returned error: {:?}, time={:?}",
                    id, result.0.get("error"), duration);
            } else {
                info!("RPC RESPONSE [id={}]: Execution layer request completed successfully, time={:?}",
                    id, duration);
            }

            result
        }
    };

    result
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
