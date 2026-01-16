//! Health check endpoint for verifying EL connectivity.

use crate::clients::el_caller::send_rpc_request;
use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;
use serde_json::json;
use std::sync::Arc;
use tracing::{debug, error};

/// Health check response with status and optional block number.
#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub block_number: Option<String>,
}

/// GET /health endpoint handler
///
/// Returns 200 OK if EL connectivity is verified via eth_blockNumber.
/// Returns 503 Service Unavailable if EL is unreachable or returns an error.
pub async fn health_check(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let el_url = state.proxy_service.get_el_url();

    let request = json!({
        "jsonrpc": "2.0",
        "method": "eth_blockNumber",
        "params": [],
        "id": 1
    });

    match send_rpc_request(&request, el_url).await {
        Ok(response) => {
            if let Some(error) = response.get("error") {
                error!("HEALTH: EL returned error: {:?}", error);
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(HealthResponse {
                        status: "unhealthy",
                        block_number: None,
                    }),
                );
            }

            let block_number = response
                .get("result")
                .and_then(|v| v.as_str())
                .map(String::from);

            debug!("HEALTH: OK, block_number={:?}", block_number);
            (
                StatusCode::OK,
                Json(HealthResponse {
                    status: "healthy",
                    block_number,
                }),
            )
        }
        Err(err) => {
            error!("HEALTH: EL connectivity check failed: {}", err);
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(HealthResponse {
                    status: "unhealthy",
                    block_number: None,
                }),
            )
        }
    }
}
