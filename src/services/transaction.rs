use crate::clients::wallet_caller::WalletCaller;
use crate::config::AppConfig;
use crate::error::AppError;
use crate::types::rpc::RpcRequest;
use crate::AppState;
use bytes::BytesMut;
use ethers::types::{Transaction, H256};
use ethers::utils::{keccak256, rlp};
use flate2::write::ZlibEncoder;
use flate2::Compression;
use serde_json::{json, Value};
use std::error::Error;
use std::io::Write;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// Structure to represent a transaction request that needs to be processed sequentially
pub struct TransactionRequest {
    pub raw_tx: String,
    pub tx_bytes: Vec<u8>,
    pub id: Value,
}

/// Creates and starts the background transaction processor
/// Returns a channel sender that can be used to queue transactions
pub fn start_transaction_processor(config: AppConfig) -> mpsc::Sender<TransactionRequest> {
    let (transaction_sender, mut transaction_receiver) = mpsc::channel::<TransactionRequest>(1024);
    info!("TX_PROCESSOR: Starting transaction processor with queue size={}", 1024);

    // Start the sequential transaction processor task
    let config = Arc::new(config);
    tokio::spawn(async move {
        info!("TX_PROCESSOR: Background worker started");

        let mut processed_count = 0;
        let mut error_count = 0;

        // Process transactions one at a time
        while let Some(tx_request) = transaction_receiver.recv().await {
            let tx_hash = compute_transaction_hash(&tx_request.tx_bytes);
            let tx_hash_str = format!("{:#x}", tx_hash);

            // Generate a proper ID string, using UUID if the original ID is null or invalid
            let id_str = if tx_request.id.is_null() {
                let uuid = Uuid::new_v4().to_string();
                info!("TX_PROCESSOR [hash={}]: Received transaction with null ID, assigning UUID: {}", tx_hash_str, uuid);
                uuid
            } else {
                tx_request.id.to_string()
            };

            info!("TX_PROCESSOR [id={}, hash={}]: Processing transaction, bytes={}",
                id_str, tx_hash_str, tx_request.tx_bytes.len());

            let start = std::time::Instant::now();

            // Create a Value with the proper ID for passing to process_wallet_call
            let id_value = if tx_request.id.is_null() {
                json!(id_str)
            } else {
                tx_request.id.clone()
            };

            // Call the wallet sequentially for each transaction
            match process_wallet_call(&tx_request.tx_bytes, &config, id_value).await {
                Ok(_) => {
                    let duration = start.elapsed();
                    processed_count += 1;
                    info!("TX_PROCESSOR [id={}, hash={}]: Transaction processed successfully, time={:?}, total_success={}, total_errors={}",
                        id_str, tx_hash_str, duration, processed_count, error_count);
                },
                Err(err) => {
                    let duration = start.elapsed();
                    error_count += 1;
                    error!("TX_PROCESSOR [id={}, hash={}]: Transaction failed: {}, time={:?}, total_success={}, total_errors={}",
                        id_str, tx_hash_str, err, duration, processed_count, error_count);
                }
            }
        }
    });

    transaction_sender
}

// Process transaction immediately but queue for sequential wallet calls
pub async fn process_transaction(req: RpcRequest, state: &Arc<AppState>) -> Value {
    // If ID is null, generate a UUID
    let id_value = if req.id.is_null() {
        let uuid = Uuid::new_v4().to_string();
        json!(uuid)
    } else {
        req.id.clone()
    };

    let id = id_value.to_string();
    info!("TX [id={}]: Processing transaction request", id);

    // Validate transaction
    let (validation_result, tx_bytes_opt) = validate_transaction(&req);

    // If validation failed, return the error
    if let Err(error_json) = validation_result {
        error!("TX [id={}]: Validation failed: {:?}", id, error_json);
        return error_json;
    }

    let tx_bytes = tx_bytes_opt.unwrap();
    debug!("TX [id={}]: Transaction validated successfully, tx_bytes_len={}", id, tx_bytes.len());

    // Compute transaction hash immediately
    let tx_hash = compute_transaction_hash(&tx_bytes);
    let tx_hash_str = format!("{:#x}", tx_hash);

    info!("TX [id={}, hash={}]: Computed hash, now queueing for background processing", id, tx_hash_str);

    // Queue the transaction for sequential processing
    let tx_request = TransactionRequest {
        raw_tx: req.params[0].as_str().unwrap_or("").to_string(),
        tx_bytes,
        id: id_value.clone(),
    };

    let queue_start = std::time::Instant::now();
    if let Err(e) = state.transaction_sender.send(tx_request).await {
        error!("TX [id={}, hash={}]: Failed to queue transaction: {}", id, tx_hash_str, e);
        // Even if queueing fails, we still return the hash to the user
    } else {
        let queue_time = queue_start.elapsed();
        info!("TX [id={}, hash={}]: Transaction queued successfully, queue_time={:?}", id, tx_hash_str, queue_time);
    }

    // Return the hash immediately
    debug!("TX [id={}, hash={}]: Returning hash to client", id, tx_hash_str);
    json!({
        "jsonrpc": "2.0",
        "result": tx_hash_str,
        "id": id_value
    })
}

/// Validates an incoming transaction request and returns the decoded transaction bytes
pub fn validate_transaction(req: &RpcRequest) -> (Result<(), Value>, Option<Vec<u8>>) {
    let id = if req.id.is_null() {
        "null".to_string()
    } else {
        req.id.to_string()
    };

    // Extract the raw transaction from the request
    let raw_tx = req.params[0].as_str().unwrap_or("");
    debug!("TX_VALIDATE [id={}]: Validating transaction, raw_tx_len={}", id, raw_tx.len());

    // Check if transaction starts with 0x
    if !raw_tx.starts_with("0x") {
        warn!("TX_VALIDATE [id={}]: Raw transaction doesn't start with '0x'", id);
        return (
            Err(AppError::InvalidTransactionFormat.to_json_rpc_error(req.id.clone())),
            None,
        );
    }

    // Remove "0x" prefix and pad with leading zero if the length is odd
    let hex_str = if raw_tx.len() % 2 != 0 {
        debug!("TX_VALIDATE [id={}]: Odd-length hex string, padding", id);
        format!("0{}", &raw_tx[2..])
    } else {
        raw_tx[2..].to_string()
    };

    // Decode from hex
    let tx_bytes = match hex::decode(hex_str) {
        Ok(bytes) => {
            // Compute hash after we have the bytes for better logging
            let hash = format!("{:#x}", compute_transaction_hash(&bytes));
            debug!("TX_VALIDATE [id={}, hash={}]: Hex decoded successfully, bytes_len={}", id, hash, bytes.len());
            bytes
        },
        Err(e) => {
            warn!("TX_VALIDATE [id={}]: Invalid hex format: {}", id, e);
            return (
                Err(AppError::InvalidTransactionFormat.to_json_rpc_error(req.id.clone())),
                None,
            );
        }
    };

    // Validate RLP format
    match rlp::decode::<Transaction>(&tx_bytes) {
        Ok(tx) => {
            let hash = format!("{:#x}", compute_transaction_hash(&tx_bytes));
            debug!("TX_VALIDATE [id={}, hash={}]: RLP decoded successfully, nonce={:?}, gas_price={:?}",
                id, hash, tx.nonce, tx.gas_price);
        },
        Err(e) => {
            warn!("TX_VALIDATE [id={}]: Failed to decode RLP: {}", id, e);
            return (
                Err(AppError::InvalidTransactionFormat.to_json_rpc_error(req.id.clone())),
                None,
            );
        }
    };

    // Return successful validation with decoded bytes
    let hash = format!("{:#x}", compute_transaction_hash(&tx_bytes));
    debug!("TX_VALIDATE [id={}, hash={}]: Validation successful", id, hash);
    (Ok(()), Some(tx_bytes))
}

/// Computes transaction hash from raw bytes
pub fn compute_transaction_hash(tx_bytes: &[u8]) -> H256 {
    keccak256(tx_bytes).into()
}

/// Processes a transaction through the wallet caller
pub async fn process_wallet_call(tx_bytes: &[u8], config: &AppConfig, id: Value) -> Result<Value, String> {
    let id_str = id.to_string();
    let tx_hash: H256 = compute_transaction_hash(tx_bytes);
    let tx_hash_str = format!("{:#x}", tx_hash);

    debug!("WALLET_CALL [id={}, hash={}]: Processing transaction with wallet, tx_bytes_len={}", id_str, tx_hash_str, tx_bytes.len());

    // Prepare the payload for wallet call
    debug!("WALLET_CALL [id={}, hash={}]: Preparing payload", id_str, tx_hash_str);
    let payload_result = prepare_payload(tx_bytes);
    if let Err(err) = payload_result {
        let error_msg = format!("Failed to prepare payload: {}", err);
        error!("WALLET_CALL [id={}, hash={}]: {}", id_str, tx_hash_str, error_msg);
        return Err(error_msg);
    }
    let payload = payload_result.unwrap();
    debug!("WALLET_CALL [id={}, hash={}]: Payload prepared successfully, size={} bytes", id_str, tx_hash_str, payload.len());

    // Call the KASPA Wallet for sending the transaction to the Base Layer
    info!("WALLET_CALL [id={}, hash={}]: Connecting to KASPA Wallet at {}", id_str, tx_hash_str, config.wallet.wallet_daemon_uri);
    let start = std::time::Instant::now();

    let wallet_caller_result = WalletCaller::new(config.wallet.clone()).await;
    if let Err(err) = wallet_caller_result {
        let error_msg = format!("Failed to create WalletCaller: {}", err);
        error!("WALLET_CALL [id={}, hash={}]: {}", id_str, tx_hash_str, error_msg);
        return Err(error_msg);
    }

    let connect_time = start.elapsed();
    let wallet_caller = wallet_caller_result.unwrap();
    debug!("WALLET_CALL [id={}, hash={}]: Connected to wallet successfully, connect_time={:?}", id_str, tx_hash_str, connect_time);

    // Actually send the transaction
    info!("WALLET_CALL [id={}, hash={}]: Sending transaction to wallet", id_str, tx_hash_str);
    let send_start = std::time::Instant::now();

    if let Err(err) = wallet_caller.send_transaction(
        payload,
        Some(id_str.clone()),
        Some(tx_hash_str.clone())
    ).await {
        let error_msg = format!("KASPA Wallet call failed: {}", err);
        let duration = send_start.elapsed();
        error!("WALLET_CALL [id={}, hash={}]: Send failed: {}, time={:?}", id_str, tx_hash_str, error_msg, duration);
        return Err(error_msg);
    }

    let send_time = send_start.elapsed();
    let total_time = start.elapsed();

    info!("WALLET_CALL [id={}, hash={}]: Transaction accepted by wallet, send_time={:?}, total_time={:?}",
        id_str, tx_hash_str, send_time, total_time);

    // Create success response with hash
    let response = json!({
        "jsonrpc": "2.0",
        "result": tx_hash_str,
        "id": id
    });

    Ok(response)
}

pub fn prepare_payload(tx_bytes: &[u8]) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
    // Compress the transaction bytes using zlib
    let mut zlib_encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    zlib_encoder.write_all(tx_bytes)?;
    let zipped_payload = zlib_encoder.finish()?;

    // Construct the payload buffer with the required header
    let mut payload_buffer = BytesMut::with_capacity(3 + zipped_payload.len());
    payload_buffer.extend_from_slice(&[0x97, 0xB1]);
    payload_buffer.extend_from_slice(&[0xA2]);
    payload_buffer.extend_from_slice(&zipped_payload);

    Ok(payload_buffer.to_vec())
}
