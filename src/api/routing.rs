use crate::{
    error::AppError,
    services::transaction,
    types::{rpc::RpcRequest, whitelist},
    AppState,
};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Instant;
use tracing::{error, info, warn};

/// Context information for request processing and logging
pub(crate) struct RequestContext {
    pub(crate) method: String,
    pub(crate) id: String,
    pub(crate) payload_info: PayloadInfo,
}

/// Information about request payload for logging
pub(crate) struct PayloadInfo {
    pub(crate) params_summary: String,
    pub(crate) estimated_size: usize,
}

impl RequestContext {
    pub(crate) fn new(req: &RpcRequest) -> Self {
        let method = req.method.clone();
        let id = req.id.to_string();
        let payload_info = PayloadInfo::extract_from_request(req);

        Self {
            method,
            id,
            payload_info,
        }
    }
}

impl PayloadInfo {
    pub(crate) fn extract_from_request(req: &RpcRequest) -> Self {
        let params_summary = match req.params.get(0) {
            Some(param) => param.to_string(),
            None => "empty".to_string(),
        };

        let estimated_size = req
            .params
            .get(0)
            .and_then(|v| v.as_str())
            .map(|s| (s.len() / 2).saturating_sub(1)) // Rough estimate: hex string / 2 - 1 for 0x
            .unwrap_or(0);

        Self {
            params_summary,
            estimated_size,
        }
    }
}

/// Core routing logic -- single source of truth for HTTP and WS.
/// Validates, routes, and logs a single JSON-RPC request.
pub async fn route_and_process(state: &Arc<AppState>, req: RpcRequest) -> Value {
    let ctx = RequestContext::new(&req);
    log_incoming_request(&ctx);

    if let Some(error_response) = validate_request_authorization(state, &req, &ctx) {
        return error_response;
    }

    let start_time = Instant::now();
    let result = route_request_to_service(state, req, &ctx).await;
    log_response(&ctx, &result, start_time.elapsed());
    result
}

/// Construct a JSON-RPC error object
pub fn json_rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "error": { "code": code, "message": message },
        "id": id
    })
}

/// Log incoming request with appropriate detail level
fn log_incoming_request(ctx: &RequestContext) {
    info!(
        "RPC REQUEST [id={}]: Received method={}",
        ctx.id, ctx.method
    );

    if ctx.method == "eth_sendRawTransaction" {
        info!(
            "RPC REQUEST [id={}]: Processing transaction, params={}, est_payload_size={} bytes",
            ctx.id, ctx.payload_info.params_summary, ctx.payload_info.estimated_size
        );
    }
}

/// Validate request authorization using whitelist if enabled and check read-only mode
fn validate_request_authorization(
    state: &Arc<AppState>,
    req: &RpcRequest,
    ctx: &RequestContext,
) -> Option<Value> {
    // Check whitelist first if enabled
    if state.config.security.enable_whitelist && !whitelist::is_method_allowed(&ctx.method) {
        warn!("Unauthorized RPC method call attempted: {}", ctx.method);
        return Some(
            AppError::MethodNotAllowed(ctx.method.clone()).to_json_rpc_error(req.id.clone()),
        );
    }

    // Check read-only mode for write methods
    if state.config.security.is_read_only() && whitelist::is_write_method(&ctx.method) {
        error!(method = %ctx.method, "Write method attempted in read-only mode");
        return Some(AppError::ReadOnlyMode.to_json_rpc_error(req.id.clone()));
    }

    None
}

/// Route request to the appropriate service based on method
async fn route_request_to_service(
    state: &Arc<AppState>,
    req: RpcRequest,
    ctx: &RequestContext,
) -> Value {
    match ctx.method.as_str() {
        "eth_sendRawTransaction" => {
            info!(
                "RPC REQUEST [id={}]: Routing to transaction service",
                ctx.id
            );
            transaction::process_transaction(req, state.clone()).await
        }
        _ => {
            info!(
                "RPC REQUEST [id={}]: Routing to proxy service for EL forwarding",
                ctx.id
            );
            state.proxy_service.forward_to_el(req).await.0
        }
    }
}

/// Validate a request's authorization (whitelist + read-only) without routing it.
/// Returns an error Value if the request should be rejected, None if it's allowed.
/// Used by the WS handler to validate subscription methods before forwarding to reth.
pub(crate) fn validate_request(state: &Arc<AppState>, req: &RpcRequest) -> Option<Value> {
    let ctx = RequestContext::new(req);
    validate_request_authorization(state, req, &ctx)
}

/// Log response with appropriate detail level
fn log_response(ctx: &RequestContext, result: &Value, duration: std::time::Duration) {
    if let Some(result_value) = result.get("result") {
        if ctx.method == "eth_sendRawTransaction" {
            let tx_hash = result_value.as_str().unwrap_or("unknown");
            info!(
                "RPC RESPONSE [id={}, hash={}]: Transaction accepted (queued), time={:?}, payload_size={} bytes",
                ctx.id, tx_hash, duration, ctx.payload_info.estimated_size
            );
        } else {
            info!(
                "RPC RESPONSE [id={}]: Request completed successfully, time={:?}",
                ctx.id, duration
            );
        }
    } else if let Some(error) = result.get("error") {
        if ctx.method == "eth_sendRawTransaction" {
            error!(
                "RPC RESPONSE [id={}]: Transaction processing failed, error={}, time={:?}, payload={}",
                ctx.id, error, duration, ctx.payload_info.params_summary
            );
        } else {
            error!(
                "RPC RESPONSE [id={}]: Request failed, error={:?}, time={:?}",
                ctx.id, error, duration
            );
        }
    }
}
