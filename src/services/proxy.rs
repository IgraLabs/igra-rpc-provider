use crate::clients::el_caller::send_rpc_request;
use crate::types::rpc::RpcRequest;
use axum::Json;
use serde_json::{json, to_value, Value};
use tracing::debug;

/// Forwards JSON-RPC requests to the IGRA EL Client.
///
/// # Arguments
/// - `req`: The incoming JSON-RPC request to forward.
/// - `rpc_url`: The URL of the IGRA EL Client's RPC interface.
///
/// # Returns
/// A JSON response either containing the forwarded result or an error message.
pub async fn forward_to_el(req: RpcRequest, rpc_url: &str) -> Json<Value> {
    // Attempt to serialize the `RpcRequest` into a `serde_json::Value`.
    let req_value = match to_value(&req) {
        Ok(value) => value,
        Err(err) => {
            let error_message = format!("Serialization error: {}", err);
            return Json(json!({ "error": error_message }));
        }
    };

    debug!(?req_value, "req_value");

    // Forward the serialized request to the IGRA EL Client.
    match send_rpc_request(&req_value, rpc_url).await {
        Ok(response) => Json(response),
        Err(err) => {
            let error_message = format!("Request failed: {}", err);
            Json(json!({ "error": error_message }))
        }
    }
}
