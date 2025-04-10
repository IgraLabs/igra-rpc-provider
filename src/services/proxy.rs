use crate::clients::el_caller::send_rpc_request;
use crate::types::rpc::RpcRequest;
use axum::Json;
use serde_json::{json, to_value, Value};
use tracing::{debug, error, info};

/// Forwards JSON-RPC requests to the IGRA EL Client.
///
/// # Arguments
/// - `req`: The incoming JSON-RPC request to forward.
/// - `rpc_url`: The URL of the IGRA EL Client's RPC interface.
///
/// # Returns
/// A JSON response either containing the forwarded result or an error message.
pub async fn forward_to_el(req: RpcRequest, rpc_url: &str) -> Json<Value> {
    let method = req.method.clone();
    let id = req.id.to_string();

    info!("PROXY [id={}]: Forwarding method={} to EL at {}", id, method, rpc_url);

    // Attempt to serialize the `RpcRequest` into a `serde_json::Value`.
    let req_value = match to_value(&req) {
        Ok(value) => value,
        Err(err) => {
            let error_message = format!("Serialization error: {}", err);
            error!("PROXY [id={}]: Failed to serialize request: {}", id, error_message);
            return Json(json!({
                "jsonrpc": "2.0",
                "error": {
                    "code": -32700,
                    "message": error_message
                },
                "id": req.id
            }));
        }
    };

    // Preview the parameters for logging
    let params_preview = match req.params.as_array() {
        Some(params) if !params.is_empty() => {
            format!("[{} items]", params.len())
        },
        _ => "[]".to_string()
    };

    debug!("PROXY [id={}]: Request serialized, method={}, params={}", id, method, params_preview);

    let start = std::time::Instant::now();

    // Forward the serialized request to the IGRA EL Client.
    match send_rpc_request(&req_value, rpc_url).await {
        Ok(response) => {
            let duration = start.elapsed();

            // Log different response types appropriately
            if let Some(error) = response.get("error") {
                error!("PROXY [id={}]: EL returned error: {:?}, time={:?}",
                    id, error, duration);
            } else if let Some(result) = response.get("result") {
                let result_type = if result.is_object() {
                    "object"
                } else if result.is_array() {
                    "array"
                } else if result.is_string() {
                    "string"
                } else if result.is_null() {
                    "null"
                } else {
                    "other"
                };

                info!("PROXY [id={}]: EL request succeeded, result_type={}, time={:?}",
                    id, result_type, duration);
            } else {
                info!("PROXY [id={}]: EL request completed, time={:?}", id, duration);
            }

            Json(response)
        },
        Err(err) => {
            let error_message = format!("Request failed: {}", err);
            let duration = start.elapsed();

            error!("PROXY [id={}]: Failed to call EL: {}, time={:?}",
                id, error_message, duration);

            Json(json!({
                "jsonrpc": "2.0",
                "error": {
                    "code": -32000,
                    "message": error_message
                },
                "id": req.id
            }))
        }
    }
}
