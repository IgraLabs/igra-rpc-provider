use crate::{
    config::AppConfig,
    services::{proxy, transaction},
    types::rpc::RpcRequest,
};
use axum::{
    extract::{Json, State},
    response::IntoResponse,
};

/// Handles JSON-RPC requests and routes them to the appropriate handler.
pub async fn handle_rpc(
    State(config): State<AppConfig>,
    Json(req): Json<RpcRequest>,
) -> impl IntoResponse {
    match req.method.as_str() {
        "eth_sendRawTransaction" => transaction::handle_send_raw_transaction(req, &config).await,
        _ => proxy::forward_to_el(req, &config.el.rpc_url).await,
    }
}
