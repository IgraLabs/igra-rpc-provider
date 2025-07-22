use crate::clients::el_caller::send_rpc_request;
use crate::types::rpc::RpcRequest;
use axum::Json;
use serde_json::{json, to_value, Value};
use tracing::{debug, error, info};

/// Service responsible for forwarding RPC requests to the EL client
/// Single responsibility: Handle HTTP request/response forwarding only
#[derive(Clone)]
pub struct ProxyService {
    el_url: String,
}

impl ProxyService {
    /// Creates a new ProxyService that forwards requests to the specified EL URL
    pub fn new(el_url: String) -> Self {
        Self { el_url }
    }

    /// Forward JSON-RPC request to the EL client without any modifications
    /// Returns the raw response from the EL client
    pub async fn forward_to_el(&self, req: RpcRequest) -> Json<Value> {
        let method = req.method.clone();
        let id = req.id.to_string();

        info!(
            "PROXY [id={}]: Forwarding method={} to EL at {}",
            id, method, self.el_url
        );

        // Serialize the request
        let req_value = match to_value(&req) {
            Ok(value) => value,
            Err(err) => {
                let error_message = format!("Request serialization failed: {}", err);
                error!("PROXY [id={}]: Serialization error: {}", id, error_message);
                return Json(json!({
                    "jsonrpc": "2.0",
                    "error": { "code": -32700, "message": error_message },
                    "id": req.id
                }));
            }
        };

        // Log request details
        let params_preview = match req.params.as_array() {
            Some(params) if !params.is_empty() => format!("[{} items]", params.len()),
            _ => "[]".to_string(),
        };

        debug!(
            "PROXY [id={}]: Forwarding request - method={}, params={}",
            id, method, params_preview
        );

        let start = std::time::Instant::now();

        // Forward the serialized request to the IGRA EL Client.
        match send_rpc_request(&req_value, &self.el_url).await {
            Ok(response) => {
                let duration = start.elapsed();

                // Log different response types appropriately
                if let Some(error) = response.get("error") {
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

                Json(response)
            }
            Err(err) => {
                let error_message = format!("EL request failed: {}", err);
                let duration = start.elapsed();

                error!(
                    "PROXY [id={}]: EL communication failed: {}, duration={:?}",
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

    /// Get the configured EL URL
    pub fn get_el_url(&self) -> &str {
        &self.el_url
    }

    /// Update the EL URL (useful for failover scenarios)
    pub fn update_el_url(&mut self, new_url: String) {
        info!(
            "PROXY: Updating EL URL from '{}' to '{}'",
            self.el_url, new_url
        );
        self.el_url = new_url;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    async fn test_proxy_forwards_request_without_modification() {
        // Arrange
        let server = MockServer::start().await;
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

        let proxy_service = ProxyService::new(server.uri());
        let request = create_test_request("eth_blockNumber", json!([]));

        // Act
        let response = proxy_service.forward_to_el(request).await;

        // Assert
        // The response should be exactly what the mock server sent, with no modifications
        assert_eq!(response.0["result"], "0x123abc");
        assert_eq!(response.0["jsonrpc"], "2.0");
        assert_eq!(response.0["id"], 1);
    }

    #[tokio::test]
    async fn test_proxy_handles_el_errors() {
        // Arrange
        let server = MockServer::start().await;
        let error_response = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {
                "code": -32601,
                "message": "Method not found"
            }
        });

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(error_response))
            .mount(&server)
            .await;

        let proxy_service = ProxyService::new(server.uri());
        let request = create_test_request("invalid_method", json!([]));

        // Act
        let response = proxy_service.forward_to_el(request).await;

        // Assert
        // Error responses should be forwarded as-is
        assert!(response.0.get("error").is_some());
        assert_eq!(response.0["error"]["code"], -32601);
        assert_eq!(response.0["error"]["message"], "Method not found");
    }

    #[tokio::test]
    async fn test_proxy_handles_network_errors() {
        // Arrange - no mock server, so connection will fail
        let proxy_service = ProxyService::new("http://invalid-url:9999".to_string());
        let request = create_test_request("eth_blockNumber", json!([]));

        // Act
        let response = proxy_service.forward_to_el(request).await;

        // Assert
        // Network errors should be converted to JSON-RPC errors
        assert!(response.0.get("error").is_some());
        assert_eq!(response.0["error"]["code"], -32000);
        assert!(response.0["error"]["message"]
            .as_str()
            .expect("message should be a string")
            .contains("EL request failed"));
    }

    #[test]
    fn test_update_el_url() {
        // Arrange
        let mut proxy_service = ProxyService::new("http://old-url".to_string());

        // Act
        proxy_service.update_el_url("http://new-url".to_string());

        // Assert
        assert_eq!(proxy_service.get_el_url(), "http://new-url");
    }
}
