use crate::clients::el_caller::send_rpc_request;
use crate::services::gas_price::GasPriceService;
use crate::types::rpc::RpcRequest;
use axum::Json;
use serde_json::{json, to_value, Value};
use std::sync::Arc;
use tracing::{debug, error, info};

#[derive(Clone)]
pub struct ProxyService {
    el_url: String,
    gas_price_service: Arc<GasPriceService>,
}

impl ProxyService {
    /// Creates a new `ProxyService`.
    pub fn new(el_url: String, gas_price_service: Arc<GasPriceService>) -> Self {
        Self {
            el_url,
            gas_price_service,
        }
    }

    /// Forwards JSON-RPC requests to the IGRA EL Client, with gas price flooring.
    pub async fn forward_to_el(&self, req: RpcRequest) -> Json<Value> {
        let method = req.method.clone();
        let id = req.id.to_string();

        info!(
            "PROXY [id={}]: Forwarding method={} to EL at {}",
            id, method, self.el_url
        );

        let req_value = match to_value(&req) {
            Ok(value) => value,
            Err(err) => {
                let error_message = format!("Serialization error: {}", err);
                error!(
                    "PROXY [id={}]: Failed to serialize request: {}",
                    id, error_message
                );
                return Json(json!({
                    "jsonrpc": "2.0",
                    "error": { "code": -32700, "message": error_message },
                    "id": req.id
                }));
            }
        };

        let params_preview = match req.params.as_array() {
            Some(params) if !params.is_empty() => format!("[{} items]", params.len()),
            _ => "[]".to_string(),
        };

        debug!(
            "PROXY [id={}]: Request serialized, method={}, params={}",
            id, method, params_preview
        );

        let start = std::time::Instant::now();

        // Forward the serialized request to the IGRA EL Client.
        match send_rpc_request(&req_value, &self.el_url).await {
            Ok(response) => {
                let duration = start.elapsed();
                let mut final_response = response;

                // If the method is `eth_gasPrice`, floor the result in-place.
                if method == "eth_gasPrice" {
                    info!("PROXY [id={}]: Intercepting eth_gasPrice response", id);
                    self.gas_price_service
                        .floor_gas_price_value(&mut final_response);
                }

                // Log different response types appropriately
                if let Some(error) = final_response.get("error") {
                    error!(
                        "PROXY [id={}]: EL returned error: {:?}, time={:?}",
                        id, error, duration
                    );
                } else {
                    info!(
                        "PROXY [id={}]: EL request succeeded, time={:?}",
                        id, duration
                    );
                }

                Json(final_response)
            }
            Err(err) => {
                let error_message = format!("Request failed: {}", err);
                let duration = start.elapsed();

                error!(
                    "PROXY [id={}]: Failed to call EL: {}, time={:?}",
                    id, error_message, duration
                );

                Json(json!({
                    "jsonrpc": "2.0",
                    "error": { "code": -32000, "message": error_message },
                    "id": req.id
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GasConfig;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // Helper to create a test RpcRequest
    fn create_test_request(method: &str, params: Value) -> RpcRequest {
        RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: json!(1),
            method: method.to_string(),
            params,
        }
    }

    #[tokio::test]
    async fn test_eth_gasprice_is_intercepted_and_floored() {
        // Arrange
        // 1. Start a mock server to act as our fake reth node.
        let server = MockServer::start().await;

        // 2. Configure the mock server to return a low gas price.
        let low_price_hex = "0x1"; // 1 Wei
        let mock_response = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": low_price_hex
        });
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(mock_response))
            .mount(&server)
            .await;

        // 3. Create our services, configured with a high price floor.
        let gas_config = GasConfig {
            min_base_fee_gwei: 100,
        }; // 100 Gwei floor
        let gas_service = Arc::new(GasPriceService::new(gas_config));
        let proxy_service = ProxyService::new(server.uri(), gas_service);
        let request = create_test_request("eth_gasPrice", json!([]));

        // Act
        let response = proxy_service.forward_to_el(request).await;

        // Assert
        // The price should be floored to 100 Gwei, not the 1 Wei from the mock server.
        let expected_floored_price = 100 * 1_000_000_000u128;
        let expected_hex = format!("0x{:x}", expected_floored_price);
        assert_eq!(response.0["result"], expected_hex);
    }

    #[tokio::test]
    async fn test_other_methods_are_not_intercepted() {
        // Arrange
        // 1. Start the mock server.
        let server = MockServer::start().await;

        // 2. Configure it to return a specific block number.
        let mock_response = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": "0x123abc"
        });
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(mock_response))
            .mount(&server)
            .await;

        // 3. Create services with a gas floor that should NOT be used.
        let gas_config = GasConfig {
            min_base_fee_gwei: 100,
        };
        let gas_service = Arc::new(GasPriceService::new(gas_config));
        let proxy_service = ProxyService::new(server.uri(), gas_service);
        let request = create_test_request("eth_blockNumber", json!([]));

        // Act
        let response = proxy_service.forward_to_el(request).await;

        // Assert
        // The response should be exactly what the mock server sent, with no modifications.
        assert_eq!(response.0["result"], "0x123abc");
    }
}
