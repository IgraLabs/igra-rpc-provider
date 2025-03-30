use crate::clients::wallet_caller::WalletCaller;
use crate::config::AppConfig;
use crate::error::AppError;
use crate::types::rpc::RpcRequest;
use axum::Json;
use bytes::BytesMut;
use ethers::types::{Transaction, H256};
use ethers::utils::{keccak256, rlp};
use flate2::write::ZlibEncoder;
use flate2::Compression;
use serde_json::{json, Value};
use std::error::Error;
use std::io::Write;
use tracing::{debug, error, info, warn};

/// Handles `eth_sendRawTransaction` requests.
pub async fn handle_send_raw_transaction(req: RpcRequest, config: &AppConfig) -> Json<Value> {
    // 1. Extract the raw transaction from the request and decode it
    let raw_tx = req.params[0].as_str().unwrap_or("");
    info!("Received `eth_sendRawTransaction` request");

    if !raw_tx.starts_with("0x") {
        warn!("Raw transaction does not start with '0x'");
        return Json(AppError::InvalidTransactionFormat.to_json_rpc_error(req.id));
    }

    // Remove "0x" prefix and pad with leading zero if the length is odd
    let hex_str = if raw_tx.len() % 2 != 0 {
        format!("0{}", &raw_tx[2..])
    } else {
        raw_tx[2..].to_string()
    };

    let tx_bytes = match hex::decode(hex_str) {
        Ok(bytes) => bytes,
        Err(_) => {
            warn!("Invalid transaction format");
            return Json(AppError::InvalidTransactionFormat.to_json_rpc_error(req.id));
        }
    };

    let tx: Transaction = match rlp::decode(&tx_bytes) {
        Ok(tx) => tx,
        Err(_) => {
            warn!("Failed to decode RLP transaction");
            return Json(AppError::InvalidTransactionFormat.to_json_rpc_error(req.id));
        }
    };

    debug!(?tx, "Decoded transaction");

    let payload_result = prepare_payload(&tx_bytes);
    if let Err(err) = payload_result {
        error!("Failed to create WalletCaller: {}", err);
        return Json(AppError::WalletCallError.to_json_rpc_error(req.id));
    }
    let payload = payload_result.unwrap();

    // 2. Call the KASPA Wallet for sending the transaction to the Base Layer
    info!("Calling the KASPA Wallet to submit a transaction");
    let wallet_caller_result = WalletCaller::new(config.wallet.clone()).await;
    if let Err(err) = wallet_caller_result {
        error!("Failed to create WalletCaller: {}", err);
        return Json(AppError::WalletCallError.to_json_rpc_error(req.id));
    }
    let wallet_caller = wallet_caller_result.unwrap();

    if let Err(err) = wallet_caller.send_transaction(payload).await {
        error!("KASPA Wallet call failed: {}", err);
        return Json(AppError::WalletCallError.to_json_rpc_error(req.id));
    }

    // 3. Compute the transaction hash
    let tx_hash: H256 = keccak256(&tx_bytes).into();
    info!("Transaction accepted successfully: tx_hash={}", tx_hash);

    // 4. Respond with the computed transaction hash
    Json(json!({
        "jsonrpc": "2.0",
        "result": format!("{:#x}", tx_hash),
        "id": req.id
    }))
}

fn prepare_payload(tx_bytes: &[u8]) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
    let mut zlib_encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    zlib_encoder.write_all(tx_bytes)?;
    let zipped_payload = zlib_encoder.finish()?;

    let mut payload_buffer = BytesMut::with_capacity(3 + zipped_payload.len());

    payload_buffer.extend_from_slice(&[0x97, 0xB1]);
    payload_buffer.extend_from_slice(&[0xA2]);
    payload_buffer.extend_from_slice(&tx_bytes);

    Ok(zipped_payload)
}
