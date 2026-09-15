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
use futures_util::{stream::FuturesUnordered, SinkExt, StreamExt};
use serde_json::Value;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, OwnedSemaphorePermit};
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite;
use tracing::{error, info, warn};

/// Maximum concurrent WebSocket connections.
pub const MAX_WS_CONNECTIONS: usize = 1024;

/// Maximum concurrent in-flight RPC requests per WebSocket connection.
const MAX_INFLIGHT_REQUESTS: usize = 64;

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
///
/// Note: non-subscription responses may arrive out of order relative to the
/// request sequence, as they are processed concurrently. Clients must match
/// responses by their JSON-RPC `id` field.
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

    // Track spawned RPC tasks for cleanup and panic detection
    let mut in_flight = JoinSet::new();
    let inflight_semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_INFLIGHT_REQUESTS));

    // Main loop: read from client with idle timeout
    loop {
        // Drain completed tasks and log any panics
        while let Some(result) = in_flight.try_join_next() {
            if let Err(e) = result {
                if e.is_panic() {
                    error!(conn_id, "Spawned RPC task panicked: {e}");
                }
            }
        }

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
                if is_subscription_method(&req.method) {
                    // Subscriptions need &mut reth_write -- must stay sequential
                    let connection_ok =
                        process_subscription_request(&state, req, &mut reth_write, &client_tx)
                            .await;
                    if !connection_ok {
                        break;
                    }
                } else {
                    // Non-subscription requests are spawned concurrently so the
                    // main loop can immediately read the next message.
                    // Use try_acquire to avoid blocking the main loop (which
                    // would stall subscription message reads).
                    let permit = match inflight_semaphore.clone().try_acquire_owned() {
                        Ok(permit) => permit,
                        Err(_) => {
                            let error_response = routing::json_rpc_error(
                                req.id.clone(),
                                -32005,
                                "Server busy, too many in-flight requests",
                            );
                            if !send_value(&client_tx, &error_response).await {
                                break;
                            }
                            continue;
                        }
                    };
                    let state = Arc::clone(&state);
                    let client_tx = client_tx.clone();
                    in_flight.spawn(async move {
                        let response = routing::route_and_process(&state, req).await;
                        let _ = send_value(&client_tx, &response).await;
                        drop(permit); // held until task completes
                    });
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

                // Reserve capacity for the concurrent request items in this batch.
                #[allow(clippy::cast_possible_truncation)] // MAX_INFLIGHT_REQUESTS (64) fits in u32
                let batch_len = requests.len().min(MAX_INFLIGHT_REQUESTS) as u32;
                let permit = match inflight_semaphore.clone().try_acquire_many_owned(batch_len) {
                    Ok(permit) => permit,
                    Err(_) => {
                        let error_response = routing::json_rpc_error(
                            Value::Null,
                            -32005,
                            "Server busy, too many in-flight requests",
                        );
                        if !send_value(&client_tx, &error_response).await {
                            break;
                        }
                        continue;
                    }
                };
                let state = Arc::clone(&state);
                let client_tx = client_tx.clone();
                in_flight.spawn(async move {
                    let futs = requests
                        .into_iter()
                        .map(|req| process_ws_request_value(&state, req));
                    process_ws_batch(futs, client_tx, permit).await;
                });
            }
        }
    }

    // Clean up: abort in-flight tasks, close reth WS, signal writer, abort relay
    in_flight.abort_all();
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

/// Tag a batch response with its request index so it can be placed back in request order.
///
/// Driving these futures through `stream::iter(..).buffer_unordered(n)` does not compile:
/// the batch futures borrow `AppState`, and any index-tagging `map` closure over that borrow
/// is rejected with `implementation of FnOnce is not general enough`. That is why
/// `process_ws_batch` primes and refills a `FuturesUnordered` by hand instead. Inside that
/// loop an inline closure does compile; this named function is kept for readability.
async fn indexed_response<F>(index: usize, request: F) -> (usize, Value)
where
    F: Future<Output = Value>,
{
    (index, request.await)
}

/// Collect batch responses in request order and retain capacity through the send.
///
/// Runs at most `permit.num_permits()` requests at once, priming that many and admitting
/// one more on each completion. Refilling on completion rather than on delivery is
/// deliberate: an ordered buffer counts a finished-but-not-yet-yielded response against the
/// window, so one slow request at the head of a batch larger than the reservation would
/// starve the rest down to a single in-flight request while every permit stayed held.
/// Responses therefore arrive out of order and are sorted back into request order before
/// the array is sent.
async fn process_ws_batch<F>(
    futures: impl IntoIterator<Item = F>,
    client_tx: mpsc::Sender<String>,
    permit: OwnedSemaphorePermit,
) where
    F: Future<Output = Value>,
{
    // The caller rejects empty batches before reserving capacity.
    assert!(permit.num_permits() > 0, "batch requires reserved capacity");
    let mut requests = futures.into_iter().enumerate();
    let mut running = FuturesUnordered::new();
    for (index, request) in requests.by_ref().take(permit.num_permits()) {
        running.push(indexed_response(index, request));
    }

    // Admit one more request per completion so the window stays full.
    let mut responses = Vec::new();
    while let Some(response) = running.next().await {
        responses.push(response);
        if let Some((next_index, next_request)) = requests.next() {
            running.push(indexed_response(next_index, next_request));
        }
    }

    // Sorting rather than placing by index keeps the array exactly as long as the
    // responses actually received, with no placeholder that could reach a client.
    responses.sort_unstable_by_key(|(index, _)| *index);
    let responses: Vec<Value> = responses.into_iter().map(|(_, value)| value).collect();
    let _ = send_value(&client_tx, &Value::Array(responses)).await;
    drop(permit);
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

/// Process a subscription request (`eth_subscribe` / `eth_unsubscribe`).
/// Returns `false` if the connection should be closed (i.e. the client
/// channel is gone or the reth upstream broke).
async fn process_subscription_request(
    state: &Arc<AppState>,
    req: RpcRequest,
    reth_write: &mut RethWsWriter,
    client_tx: &mpsc::Sender<String>,
) -> bool {
    // Validate authorization before forwarding to reth
    if let Some(error_response) = routing::validate_request(state, &req) {
        return send_value(client_tx, &error_response).await;
    }
    // Forward to reth WS -- response will come back via the relay task
    forward_to_reth(reth_write, &req).await
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

/// Batch concurrency regression tests.
///
/// Affected revision: the unbounded collector was introduced in `ec10085`, was present through
/// `c840c0d` (`src/api/ws.rs` blob `8cb88ed`), and shipped in every tag from `v2.3.0` through
/// `v3.1.0` (all six resolve that path to the same blob). Whether each tag was deployed is not
/// recorded here.
///
/// `batch_respects_reserved_capacity`, `batch_cancellation_drops_active_items_and_releases_capacity`
/// and `batch_refills_window_when_a_completed_item_cannot_yet_be_yielded` are the reproducers: all
/// three fail against that revision's `join_all` collector, which polled every request in a batch
/// regardless of how many permits were reserved. The response-ordering and permit-lifetime tests
/// guard contracts that `join_all` also satisfied, and pass either way.
#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::poll;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::{oneshot, Semaphore};

    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    #[derive(Default)]
    struct Activity {
        active: AtomicUsize,
        peak: AtomicUsize,
        started: AtomicUsize,
    }

    struct ActiveRequest(Arc<Activity>);

    impl Drop for ActiveRequest {
        fn drop(&mut self) {
            self.0.active.fetch_sub(1, Ordering::SeqCst);
        }
    }

    fn response(id: usize) -> Value {
        if id.is_multiple_of(2) {
            json!({"jsonrpc": "2.0", "id": id, "result": id})
        } else {
            routing::json_rpc_error(json!(id), -32600, "test error")
        }
    }

    fn gated_responses(
        count: usize,
        activity: Arc<Activity>,
    ) -> (
        Vec<oneshot::Sender<()>>,
        impl Iterator<Item = impl Future<Output = Value>>,
    ) {
        let (gates, futures): (Vec<_>, Vec<_>) = (0..count)
            .map(|id| {
                let (gate, ready) = oneshot::channel();
                let activity = Arc::clone(&activity);
                let future = async move {
                    let active = activity
                        .active
                        .fetch_add(1, Ordering::SeqCst)
                        .checked_add(1)
                        .expect("test active count fits in usize");
                    activity.peak.fetch_max(active, Ordering::SeqCst);
                    activity.started.fetch_add(1, Ordering::SeqCst);
                    let _active = ActiveRequest(activity);
                    ready.await.expect("gate released");
                    response(id)
                };
                (gate, future)
            })
            .unzip();
        (gates, futures.into_iter())
    }

    #[tokio::test]
    async fn batch_respects_reserved_capacity() {
        // The smaller reservation also detects accidentally hard-coding 64 in the helper.
        for (count, reserved) in [(1, 1), (63, 63), (64, 64), (65, 64), (129, 64), (65, 7)] {
            let semaphore = Arc::new(Semaphore::new(MAX_INFLIGHT_REQUESTS));
            let permit = Arc::clone(&semaphore)
                .try_acquire_many_owned(u32::try_from(reserved).expect("test capacity fits in u32"))
                .expect("capacity available");
            let activity = Arc::new(Activity::default());
            let (gates, futures) = gated_responses(count, Arc::clone(&activity));
            let (client_tx, mut client_rx) = mpsc::channel(1);
            let mut batch = Box::pin(process_ws_batch(futures, client_tx, permit));

            // Poll until every runnable item is waiting on its persistent gate.
            assert!(poll!(batch.as_mut()).is_pending());
            assert_eq!(
                activity.active.load(Ordering::SeqCst),
                reserved,
                "batch of {count} exceeded its reservation"
            );
            for gate in gates {
                gate.send(()).expect("request still waiting");
            }
            tokio::time::timeout(TEST_TIMEOUT, batch)
                .await
                .expect("batch completes");

            assert!(activity.peak.load(Ordering::SeqCst) <= reserved);
            assert_eq!(activity.started.load(Ordering::SeqCst), count);
            assert_eq!(activity.active.load(Ordering::SeqCst), 0);
            assert_eq!(semaphore.available_permits(), MAX_INFLIGHT_REQUESTS);
            let actual: Value =
                serde_json::from_str(&client_rx.try_recv().expect("one batch response"))
                    .expect("valid JSON");
            assert_eq!(actual, Value::Array((0..count).map(response).collect()));
            assert!(client_rx.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn batch_preserves_order_when_later_item_finishes_first() {
        let semaphore = Arc::new(Semaphore::new(2));
        let permit = semaphore
            .try_acquire_many_owned(2)
            .expect("capacity available");
        let activity = Arc::new(Activity::default());
        let (mut gates, futures) = gated_responses(2, Arc::clone(&activity));
        let (client_tx, mut client_rx) = mpsc::channel(1);
        let mut batch = Box::pin(process_ws_batch(futures, client_tx, permit));
        assert!(poll!(batch.as_mut()).is_pending());
        assert_eq!(activity.active.load(Ordering::SeqCst), 2);

        gates.pop().expect("second gate").send(()).expect("waiting");
        assert!(poll!(batch.as_mut()).is_pending());
        assert_eq!(activity.active.load(Ordering::SeqCst), 1);
        assert!(client_rx.try_recv().is_err());
        gates.pop().expect("first gate").send(()).expect("waiting");
        tokio::time::timeout(TEST_TIMEOUT, batch)
            .await
            .expect("batch completes");
        let actual: Value = serde_json::from_str(&client_rx.try_recv().expect("batch response"))
            .expect("valid JSON");
        assert_eq!(actual, json!([response(0), response(1)]));
        assert!(client_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn batch_refills_window_when_a_completed_item_cannot_yet_be_yielded() {
        // Guards against an ordered buffer, whose queue counts a completed-but-unyielded
        // result against the window: item 1 finishing while item 0 is still gated would
        // hold its slot and pin `started` at 2 instead of admitting item 2.
        let semaphore = Arc::new(Semaphore::new(2));
        let permit = Arc::clone(&semaphore)
            .try_acquire_many_owned(2)
            .expect("capacity available");
        let activity = Arc::new(Activity::default());
        let (mut gates, futures) = gated_responses(4, Arc::clone(&activity));
        let (client_tx, mut client_rx) = mpsc::channel(1);
        let mut batch = Box::pin(process_ws_batch(futures, client_tx, permit));

        assert!(poll!(batch.as_mut()).is_pending());
        assert_eq!(activity.started.load(Ordering::SeqCst), 2);

        // Release item 1 while item 0 is still gated; its slot must pass to item 2.
        gates.remove(1).send(()).expect("item 1 still waiting");
        assert!(poll!(batch.as_mut()).is_pending());
        assert_eq!(activity.started.load(Ordering::SeqCst), 3);
        assert_eq!(activity.active.load(Ordering::SeqCst), 2);
        assert!(client_rx.try_recv().is_err());

        for gate in gates {
            gate.send(()).expect("request still waiting");
        }
        tokio::time::timeout(TEST_TIMEOUT, batch)
            .await
            .expect("batch completes");

        assert!(activity.peak.load(Ordering::SeqCst) <= 2);
        assert_eq!(activity.started.load(Ordering::SeqCst), 4);
        assert_eq!(activity.active.load(Ordering::SeqCst), 0);
        assert_eq!(semaphore.available_permits(), 2);
        let actual: Value = serde_json::from_str(&client_rx.try_recv().expect("batch response"))
            .expect("valid JSON");
        assert_eq!(actual, Value::Array((0..4).map(response).collect()));
        assert!(client_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn batch_cancellation_drops_active_items_and_releases_capacity() {
        let semaphore = Arc::new(Semaphore::new(MAX_INFLIGHT_REQUESTS));
        let permit = Arc::clone(&semaphore)
            .try_acquire_many_owned(64)
            .expect("capacity available");
        let activity = Arc::new(Activity::default());
        let (gates, futures) = gated_responses(129, Arc::clone(&activity));
        let (client_tx, mut client_rx) = mpsc::channel(1);
        let mut batch = Box::pin(process_ws_batch(futures, client_tx, permit));
        assert!(poll!(batch.as_mut()).is_pending());
        assert_eq!(activity.active.load(Ordering::SeqCst), 64);
        assert_eq!(semaphore.available_permits(), 0);

        let mut in_flight = JoinSet::new();
        in_flight.spawn(batch);
        in_flight.abort_all();
        tokio::time::timeout(TEST_TIMEOUT, async {
            while let Some(result) = in_flight.join_next().await {
                assert!(result.expect_err("batch aborted").is_cancelled());
            }
        })
        .await
        .expect("aborted tasks drained");

        assert_eq!(activity.active.load(Ordering::SeqCst), 0);
        assert_eq!(activity.started.load(Ordering::SeqCst), 64);
        assert!(gates.iter().all(oneshot::Sender::is_closed));
        assert_eq!(semaphore.available_permits(), MAX_INFLIGHT_REQUESTS);
        assert!(client_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn batch_holds_capacity_until_blocked_send_finishes_or_channel_closes() {
        for close_channel in [false, true] {
            let semaphore = Arc::new(Semaphore::new(1));
            let permit = Arc::clone(&semaphore)
                .try_acquire_owned()
                .expect("capacity available");
            let (client_tx, mut client_rx) = mpsc::channel(1);
            client_tx
                .try_send("occupied".into())
                .expect("channel empty");
            let mut batch = Box::pin(process_ws_batch(
                [std::future::ready(response(0))],
                client_tx,
                permit,
            ));
            assert!(poll!(batch.as_mut()).is_pending());
            assert_eq!(semaphore.available_permits(), 0);

            if close_channel {
                client_rx.close();
            } else {
                assert_eq!(client_rx.try_recv().expect("queued message"), "occupied");
            }
            tokio::time::timeout(TEST_TIMEOUT, batch)
                .await
                .expect("send completes or fails");
            assert_eq!(semaphore.available_permits(), 1);
            if close_channel {
                assert_eq!(client_rx.try_recv().expect("queued message"), "occupied");
            } else {
                let actual: Value =
                    serde_json::from_str(&client_rx.try_recv().expect("batch response"))
                        .expect("valid JSON");
                assert_eq!(actual, json!([response(0)]));
            }
            assert!(client_rx.try_recv().is_err());
        }
    }
}
