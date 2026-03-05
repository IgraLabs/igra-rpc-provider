use crate::{
    api::routing,
    types::rpc::{RpcEnvelope, RpcRequest},
    AppState,
};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite;
use tracing::{error, info, warn};

/// Maximum concurrent WebSocket connections.
pub const MAX_WS_CONNECTIONS: usize = 1024;

/// Timeout for connecting to the upstream reth WebSocket.
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Close the connection if no message is received within this duration.
const WS_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Maximum time to wait for the writer task to flush during cleanup.
const WRITER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Channel buffer size for outgoing messages to the client.
const CLIENT_SEND_BUFFER: usize = 256;

/// Monotonic counter for assigning unique connection IDs to WebSocket sessions.
static CONNECTION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Axum handler that upgrades an HTTP GET request to a WebSocket connection.
pub async fn handle_ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> Response {
    let permit = match state.ws_semaphore.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            warn!("WebSocket connection limit reached");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Too many WebSocket connections",
            )
                .into_response();
        }
    };

    let conn_id = CONNECTION_COUNTER.fetch_add(1, Ordering::Relaxed);
    info!(conn_id, "WebSocket upgrade requested");
    ws.on_upgrade(move |socket| handle_ws_connection(socket, state, permit, conn_id))
        .into_response()
}

/// Manages a single WebSocket connection.
///
/// Connects to the reth WS endpoint for subscription relay and processes
/// all incoming JSON-RPC messages:
/// - `eth_subscribe` / `eth_unsubscribe` are forwarded to the reth WS connection
/// - All other methods go through the shared `routing::route_and_process()` path
async fn handle_ws_connection(
    client_ws: WebSocket,
    state: Arc<AppState>,
    _permit: tokio::sync::OwnedSemaphorePermit,
    conn_id: u64,
) {
    let el_ws_url = state.config.proxy.el_ws_url();
    info!(conn_id, url = %el_ws_url, "Connecting to reth WebSocket");

    // Connect to reth WS with timeout
    let connect_result = tokio::time::timeout(
        UPSTREAM_CONNECT_TIMEOUT,
        tokio_tungstenite::connect_async(&el_ws_url),
    )
    .await;

    let reth_ws = match connect_result {
        Ok(Ok((stream, _response))) => stream,
        Ok(Err(e)) => {
            error!(conn_id, error = %e, "Failed to connect to reth WebSocket");
            send_error_and_close(client_ws, "Backend WebSocket connection failed").await;
            return;
        }
        Err(_) => {
            error!(conn_id, "Timeout connecting to reth WebSocket");
            send_error_and_close(client_ws, "Backend WebSocket connection timeout").await;
            return;
        }
    };

    // Split both connections into read/write halves
    let (mut client_write, mut client_read) = client_ws.split();
    let (mut reth_write, mut reth_read) = reth_ws.split();

    // Channel for sending messages back to the client from multiple producers:
    // - The reth relay task (subscription events)
    // - The main loop (RPC responses)
    let (client_tx, mut client_rx) = mpsc::channel::<String>(CLIENT_SEND_BUFFER);

    // Task 1: Drain client_rx and write to client WebSocket
    let writer_task = tokio::spawn(async move {
        while let Some(text) = client_rx.recv().await {
            if client_write.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
        // Try to send a close frame when the channel is drained
        let _ = client_write.send(Message::Close(None)).await;
    });

    // Task 2: Relay reth->client (subscription events)
    let relay_tx = client_tx.clone();
    let relay_conn_id = conn_id;
    let relay_task = tokio::spawn(async move {
        while let Some(msg_result) = reth_read.next().await {
            match msg_result {
                Ok(tungstenite::Message::Text(text)) => {
                    if relay_tx.send(text.to_string()).await.is_err() {
                        break;
                    }
                }
                Ok(tungstenite::Message::Binary(data)) => match String::from_utf8(data.to_vec()) {
                    Ok(text) => {
                        if relay_tx.send(text).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        warn!(conn_id = relay_conn_id, error = %e, "Invalid UTF-8 from reth WebSocket");
                    }
                },
                Ok(tungstenite::Message::Close(_)) => break,
                Ok(tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_)) => {}
                Ok(tungstenite::Message::Frame(_)) => {}
                Err(e) => {
                    warn!(conn_id = relay_conn_id, error = %e, "Reth WebSocket read error");
                    break;
                }
            }
        }
        info!(conn_id = relay_conn_id, "Reth->client relay task ended");
    });

    // Main loop: read from client with idle timeout
    loop {
        let msg_result = match tokio::time::timeout(WS_IDLE_TIMEOUT, client_read.next()).await {
            Ok(Some(msg)) => msg,
            Ok(None) => break, // stream ended
            Err(_) => {
                info!(conn_id, "WebSocket idle timeout, closing connection");
                break;
            }
        };

        let text = match msg_result {
            Ok(Message::Text(t)) => t.to_string(),
            Ok(Message::Binary(b)) => match String::from_utf8(b.to_vec()) {
                Ok(s) => s,
                Err(e) => {
                    warn!(conn_id, error = %e, "Invalid UTF-8 in binary WS frame");
                    continue;
                }
            },
            Ok(Message::Close(_)) => break,
            Ok(Message::Ping(_) | Message::Pong(_)) => continue,
            Err(e) => {
                warn!(conn_id, error = %e, "Client WebSocket read error");
                break;
            }
        };

        // Parse the incoming text as a JSON-RPC envelope (single or batch)
        let envelope: RpcEnvelope = match serde_json::from_str(&text) {
            Ok(env) => env,
            Err(e) => {
                let error_response =
                    routing::json_rpc_error(Value::Null, -32700, &format!("Parse error: {e}"));
                if !send_value(&client_tx, &error_response).await {
                    break;
                }
                continue;
            }
        };

        match envelope {
            RpcEnvelope::Single(req) => {
                let connection_ok =
                    process_ws_request(&state, req, &mut reth_write, &client_tx).await;
                if !connection_ok {
                    break;
                }
            }
            RpcEnvelope::Batch(requests) => {
                if requests.is_empty() {
                    let error_response = routing::json_rpc_error(
                        Value::Null,
                        -32600,
                        "Invalid Request: empty batch",
                    );
                    if !send_value(&client_tx, &error_response).await {
                        break;
                    }
                    continue;
                }

                let mut responses = Vec::with_capacity(requests.len());
                for req in requests {
                    responses.push(process_ws_request_value(&state, req).await);
                }

                if !send_value(&client_tx, &Value::Array(responses)).await {
                    break;
                }
            }
        }
    }

    // Clean up: close reth WS, signal writer, abort relay
    let _ = reth_write.send(tungstenite::Message::Close(None)).await;
    drop(client_tx);
    relay_task.abort();
    if tokio::time::timeout(WRITER_SHUTDOWN_TIMEOUT, writer_task)
        .await
        .is_err()
    {
        warn!(conn_id, "Writer task did not shut down within timeout");
    }
    info!(conn_id, "WebSocket connection closed");
}

/// Send a JSON-RPC error and close frame to the client when the connection cannot be established.
async fn send_error_and_close(client_ws: WebSocket, message: &str) {
    let (mut write, _) = client_ws.split();
    let error = routing::json_rpc_error(Value::Null, -32603, message);
    if let Ok(s) = serde_json::to_string(&error) {
        let _ = write.send(Message::Text(s.into())).await;
    }
    let _ = write.send(Message::Close(None)).await;
}

type RethWsWriter = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    tungstenite::Message,
>;

/// Process a single WS request. Returns `false` if the connection should be closed
/// (i.e. the client channel is gone or the reth upstream broke).
///
/// Subscription methods (`eth_subscribe`/`eth_unsubscribe`) are forwarded directly
/// to the reth WS connection. All other methods go through `routing::route_and_process()`.
async fn process_ws_request(
    state: &Arc<AppState>,
    req: RpcRequest,
    reth_write: &mut RethWsWriter,
    client_tx: &mpsc::Sender<String>,
) -> bool {
    if is_subscription_method(&req.method) {
        // Validate authorization before forwarding to reth
        if let Some(error_response) = routing::validate_request(state, &req) {
            return send_value(client_tx, &error_response).await;
        }
        // Forward to reth WS -- response will come back via the relay task
        return forward_to_reth(reth_write, &req).await;
    }

    // Route through shared logic (same path as HTTP)
    let response = routing::route_and_process(state, req).await;
    send_value(client_tx, &response).await
}

/// Process a single WS request and return the response as a Value.
/// Used for batch processing where we collect all responses into an array.
///
/// Subscription methods inside a batch are rejected outright -- they require
/// a persistent async relay that is incompatible with batch response collection.
async fn process_ws_request_value(state: &Arc<AppState>, req: RpcRequest) -> Value {
    if is_subscription_method(&req.method) {
        let req_id = req.id.clone();
        // Reject early: subscriptions cannot be meaningfully included in a batch
        // because their responses arrive asynchronously via the relay task.
        return routing::json_rpc_error(
            req_id,
            -32600,
            "Subscription requests in batch are not supported; send eth_subscribe as a single request",
        );
    }

    routing::route_and_process(state, req).await
}

/// Serialize a Value and send it to the client channel.
/// Returns `false` if the channel is closed (connection should be torn down).
async fn send_value(client_tx: &mpsc::Sender<String>, value: &Value) -> bool {
    match serde_json::to_string(value) {
        Ok(s) => client_tx.send(s).await.is_ok(),
        Err(e) => {
            error!(error = %e, "Failed to serialize JSON-RPC response");
            true // serialization failure is not a connection error
        }
    }
}

/// Forward a JSON-RPC request to the upstream reth WebSocket.
/// Returns `false` if the upstream connection is broken.
async fn forward_to_reth(reth_write: &mut RethWsWriter, req: &RpcRequest) -> bool {
    let json_text = match serde_json::to_string(req) {
        Ok(s) => s,
        Err(e) => {
            error!(error = %e, "Failed to serialize subscription request");
            return true; // serialization failure is not a connection error
        }
    };
    if let Err(e) = reth_write.send(tungstenite::Message::Text(json_text)).await {
        error!(error = %e, "Failed to forward subscription to reth");
        return false;
    }
    true
}

/// Check if the method is a WebSocket subscription method.
fn is_subscription_method(method: &str) -> bool {
    matches!(method, "eth_subscribe" | "eth_unsubscribe")
}
