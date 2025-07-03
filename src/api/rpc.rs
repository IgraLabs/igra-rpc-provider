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
use std::sync::Arc;
use tracing::{error, info, warn};

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
                None => "empty".to_string(),
            };

            // Get payload size if possible
            let payload_size = req
                .params
                .get(0)
                .and_then(|v| v.as_str())
                .map(|s| (s.len() / 2).saturating_sub(1)) // Rough estimate: hex string / 2 - 1 for 0x
                .unwrap_or(0);

            info!(
                "RPC REQUEST [id={}]: Processing transaction, params={}, est_payload_size={} bytes",
                id, full_params, payload_size
            );

            // Process transaction and return hash immediately, with background wallet processing
            let start_time = std::time::Instant::now();
            let result = transaction::process_transaction(req, state.clone()).await;
            let duration = start_time.elapsed();

            // Extract result or error for logging
            if let Some(result_value) = result.get("result") {
                let tx_hash = result_value.as_str().unwrap_or("unknown");
                info!("RPC RESPONSE [id={}, hash={}]: Transaction processed successfully, time={:?}, payload_size={} bytes, request_payload={}",
                    id, tx_hash, duration, payload_size, full_params);
            } else if let Some(error) = result.get("error") {
                error!("RPC RESPONSE [id={}]: Transaction processing failed, error={}, time={:?}, payload={}",
                    id, error, duration, full_params);
            }

            axum::Json(result)
        }
        // For all other methods, just forward to EL using the original logic
        _ => {
            info!(
                "RPC REQUEST [id={}]: Forwarding method={} to execution layer",
                id, method
            );

            let start_time = std::time::Instant::now();
            let result = state.proxy_service.forward_to_el(req).await;
            let duration = start_time.elapsed();

            // Extract result or error for logging
            if result.0.get("error").is_some() {
                error!(
                    "RPC RESPONSE [id={}]: Execution layer returned error: {:?}, time={:?}",
                    id,
                    result.0.get("error"),
                    duration
                );
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
    use crate::config::{
        AppConfig, ElConfig, GasConfig, MiningConfig, SecurityConfig, ServerConfig, WalletConfig,
    };
    use serde_json::json;

    // Helper to create a default test config with a fake EL URL
    fn test_config(enable_whitelist: bool) -> AppConfig {
        AppConfig {
            server: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 8535,
            },
            el: ElConfig {
                url: "http://localhost:12345".to_string(),
            },
            wallet: WalletConfig {
                wallet_daemon_uri: "http://localhost:8082".to_string(),
                to_address: "".to_string(),
            },
            security: SecurityConfig { enable_whitelist },
            mining: MiningConfig::default(),
            gas: GasConfig::default(),
        }
    }

    // Direct test for whitelist validation without involving the full handler
    #[test]
    fn test_method_allowed_by_whitelist() {
        assert!(whitelist::is_method_allowed("eth_getBalance"));
        assert!(!whitelist::is_method_allowed("debug_traceTransaction"));
    }

    // Test the error response format for disallowed methods
    #[test]
    fn test_error_format_for_disallowed_method() {
        let method = "debug_traceTransaction".to_string();
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
