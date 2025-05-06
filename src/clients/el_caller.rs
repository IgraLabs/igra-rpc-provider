use crate::error::AppError;
use once_cell::sync::Lazy;
use reqwest::Client;
use serde_json::Value;
use tracing::{debug, error, info};

// A shared, pre-configured HTTP client for sending requests.
static HTTP_CLIENT: Lazy<Client> = Lazy::new(|| {
    info!("EL_CLIENT: Initializing HTTP client for Execution Layer");
    let client = Client::builder()
        .pool_max_idle_per_host(10)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("Failed to build HTTP client");

    info!("EL_CLIENT: HTTP client initialized with pool_max_idle_per_host=10, timeout=30s");
    client
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
    // Extract request ID for logging
    let req_id = match req.get("id") {
        Some(id) => id.to_string(),
        None => "unknown".to_string(),
    };

    // Extract method for logging
    let method = match req.get("method") {
        Some(m) => m.as_str().unwrap_or("unknown").to_string(),
        None => "unknown".to_string(),
    };

    debug!(
        "EL_CLIENT [id={}]: Sending method={} to {}",
        req_id, method, rpc_url
    );

    // Start timing
    let start = std::time::Instant::now();

    // Send the HTTP POST request with the JSON payload.
    let response = match HTTP_CLIENT.post(rpc_url).json(req).send().await {
        Ok(resp) => {
            let status = resp.status();
            let duration = start.elapsed();
            debug!(
                "EL_CLIENT [id={}]: Received HTTP response status={}, time={:?}",
                req_id, status, duration
            );
            resp
        }
        Err(err) => {
            let duration = start.elapsed();
            error!(
                "EL_CLIENT [id={}]: HTTP request failed: {}, time={:?}",
                req_id, err, duration
            );
            return Err(AppError::ElCallError(err));
        }
    };

    // Extract the response body as JSON.
    match response.json::<Value>().await {
        Ok(json) => {
            let duration = start.elapsed();

            if json.get("error").is_some() {
                let err_details = json
                    .get("error")
                    .expect("Error field should exist due to prior is_some() check");
                error!(
                    "EL_CLIENT [id={}]: JSON-RPC error in response: {:?}, time={:?}",
                    req_id, err_details, duration
                );
            } else {
                debug!(
                    "EL_CLIENT [id={}]: Successful response for method={}, time={:?}",
                    req_id, method, duration
                );
            }

            Ok(json)
        }
        Err(err) => {
            let duration = start.elapsed();
            error!(
                "EL_CLIENT [id={}]: Failed to parse JSON response: {}, time={:?}",
                req_id, err, duration
            );
            Err(AppError::ElCallError(err))
        }
    }
}
