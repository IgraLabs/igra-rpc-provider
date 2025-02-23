use reqwest::Client;
use serde_json::Value;
use once_cell::sync::Lazy;
use crate::error::AppError;

// A shared, pre-configured HTTP client for sending requests.
static HTTP_CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .pool_max_idle_per_host(10)
        .build()
        .expect("Failed to build HTTP client")
});

/// Sends a JSON-RPC request to the IGRA EL Client.
///
/// # Arguments
/// - `req`: The JSON-RPC request payload as a `serde_json::Value`.
/// - `rpc_url`: The URL of the RPC interface of the IGRA EL Client.
///
/// # Returns
/// Returns a `Result` wrapping the JSON-RPC response as a `serde_json::Value`
/// on success or an `AppError` on failure.
///
/// # Errors
/// - Returns `AppError::ElRpcCallError` if the RPC request fails or
///   the response cannot be parsed as JSON.
pub async fn send_rpc_request(req: &Value, rpc_url: &str) -> Result<Value, AppError> {
    // Send the HTTP POST request with the JSON payload.
    let response = HTTP_CLIENT
        .post(rpc_url)
        .json(req)
        .send()
        .await
        .map_err(AppError::ElCallError)?;

    // Extract the response body as JSON.
    let json: Value = response.json().await.map_err(AppError::ElCallError)?;
    Ok(json)
}
