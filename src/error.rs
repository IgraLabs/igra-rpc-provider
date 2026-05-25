use serde_json::{json, Value};
use thiserror::Error;

use crate::types::wallet::KaspaWalletError;

/// Custom application error type for handling various types of errors.
#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum AppError {
    /// Error indicates a failure in loading or parsing configuration.
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Error indicates an invalid L2 transaction format.
    #[error("Invalid L2 transaction format")]
    InvalidTransactionFormat,

    /// Error indicates a failure in executing a call to the IGRA EL Client.
    #[error("IGRA EL Client call failed")]
    ElCallError(#[from] reqwest::Error),

    /// Error indicates a failure in executing a call to the KASPA Wallet.
    #[error("KASPA Wallet call failed")]
    WalletCallError,

    /// Error indicates that the requested RPC method is not allowed.
    #[error("RPC method not allowed")]
    MethodNotAllowed(String),

    /// Error indicates that the data for an IGRA payload is invalid.
    #[error("Invalid IGRA payload data: {0}")]
    InvalidPayload(String),

    /// Error indicates a failure during payload serialization.
    #[error("Payload serialization error: {0}")]
    SerializationError(String),

    /// Error indicates that transaction mining exceeded the configured timeout.
    #[error("Mining timeout after {timeout_seconds} seconds")]
    MiningTimeout { timeout_seconds: u64 },

    /// Error indicates that transaction mining exhausted all possible nonces.
    #[error("Mining nonce exhaustion after trying {nonces_tried} nonces")]
    NonceExhaustion { nonces_tried: u32 },

    /// Error indicates a failure in transaction codec operations (encode/decode).
    #[error("Transaction codec error: {operation} failed - {reason}")]
    TransactionCodecError { operation: String, reason: String },

    /// Error indicates a general failure during transaction mining operations.
    #[error("Transaction mining error: {0}")]
    MiningError(String),

    /// Error indicates a failure in mining configuration or setup.
    #[error("Mining configuration error: {0}")]
    MiningConfigError(String),

    /// Error indicates that mining failed due to invalid transaction state.
    #[error("Mining invalid transaction state: {0}")]
    MiningInvalidState(String),

    /// Error indicates a wallet operation failure.
    #[error("Wallet error: {0}")]
    WalletError(String),

    /// Error indicates a JSON-RPC error.
    #[error("JSON-RPC error: {0}")]
    JsonRpcError(Value),

    /// Error indicates an internal error.
    #[error("Internal error: {0}")]
    Internal(String),

    /// Error indicates UTXO exhaustion (no funds to send).
    #[error("UTXO exhausted: no funds available to send")]
    UtxoExhausted,

    /// Error indicates retry attempts have been exhausted.
    #[error("Retry exhausted after {attempts} attempts: {reason}")]
    RetryExhausted { attempts: u32, reason: String },

    /// Error indicates that a write operation was attempted in read-only mode.
    #[error("Read-only mode is enabled")]
    ReadOnlyMode,

    /// Error indicates that a transaction failed KIP-21 lane enforcement
    /// (wrong subnetwork, pre-Toccata version, empty payload, or final tx
    /// id does not match the configured TX_ID_PREFIX).
    #[error("KIP-21 lane enforcement failed: {0}")]
    LaneEnforcementFailed(String),
}

// Add conversion from WalletError to AppError
impl From<KaspaWalletError> for AppError {
    fn from(err: KaspaWalletError) -> Self {
        match err {
            KaspaWalletError::UserInputError(msg) => {
                AppError::WalletError(format!("User input error: {msg}"))
            }
            KaspaWalletError::InternalServerError(msg) => {
                AppError::WalletError(format!("Internal server error: {msg}"))
            }
        }
    }
}

impl AppError {
    /// Converts the application error into a JSON-RPC error object.
    ///
    /// # Arguments
    /// - `id`: The JSON-RPC `id` used to associate the error with the request.
    ///
    /// # Returns
    /// A `serde_json::Value` object representing the JSON-RPC error response.
    pub fn to_json_rpc_error(&self, id: Value) -> Value {
        let (code, message) = match self {
            AppError::ConfigError(s) => (-32000, format!("Configuration error: {s}")),
            AppError::InvalidTransactionFormat => {
                (-32001, "Invalid L2 transaction format".to_string())
            }
            AppError::ElCallError(_) => (-32000, "IGRA EL Client call failed".to_string()),
            AppError::WalletCallError => (-32005, "KASPA Wallet call failed".to_string()),
            AppError::MethodNotAllowed(method) => {
                (-32002, format!("RPC method not allowed: {method}"))
            }
            AppError::InvalidPayload(reason) => (-32003, format!("Invalid IGRA payload: {reason}")),
            AppError::SerializationError(reason) => {
                (-32004, format!("Payload serialization error: {reason}"))
            }
            AppError::MiningTimeout { timeout_seconds } => (
                -32007,
                format!("Transaction mining timeout after {timeout_seconds} seconds"),
            ),
            AppError::NonceExhaustion { nonces_tried } => (
                -32008,
                format!("Transaction mining exhausted {nonces_tried} nonces"),
            ),
            AppError::TransactionCodecError { operation, reason } => (
                -32009,
                format!("Transaction codec {operation} failed: {reason}"),
            ),
            AppError::MiningError(reason) => {
                (-32006, format!("Transaction mining error: {reason}"))
            }
            AppError::MiningConfigError(reason) => {
                (-32010, format!("Mining configuration error: {reason}"))
            }
            AppError::MiningInvalidState(reason) => (
                -32011,
                format!("Mining invalid transaction state: {reason}"),
            ),
            AppError::WalletError(reason) => (-32012, format!("Wallet error: {reason}")),
            AppError::JsonRpcError(json_error) => (-32000, format!("JSON-RPC error: {json_error}")),
            AppError::Internal(reason) => (-32000, format!("Internal error: {reason}")),
            AppError::UtxoExhausted => (
                -32014,
                "UTXO exhausted: no funds available to send".to_string(),
            ),
            AppError::RetryExhausted { attempts, reason } => (
                -32015,
                format!("Retry exhausted after {attempts} attempts: {reason}"),
            ),
            AppError::ReadOnlyMode => (-32000, "Read-only mode is enabled".to_string()),
            AppError::LaneEnforcementFailed(reason) => {
                (-32016, format!("KIP-21 lane enforcement failed: {reason}"))
            }
        };

        json!({
            "jsonrpc": "2.0",
            "error": {
                "code": code,
                "message": message
            },
            "id": id
        })
    }

    /// Creates a mining timeout error with performance context
    pub fn mining_timeout(timeout_seconds: u64) -> Self {
        Self::MiningTimeout { timeout_seconds }
    }

    /// Creates a nonce exhaustion error with performance context
    pub fn nonce_exhaustion(nonces_tried: u32) -> Self {
        Self::NonceExhaustion { nonces_tried }
    }

    /// Creates a transaction codec error with operation context
    pub fn transaction_codec_error(operation: &str, reason: &str) -> Self {
        Self::TransactionCodecError {
            operation: operation.to_string(),
            reason: reason.to_string(),
        }
    }

    /// Creates a mining configuration error
    pub fn mining_config_error(reason: &str) -> Self {
        Self::MiningConfigError(reason.to_string())
    }

    /// Creates a mining invalid state error
    pub fn mining_invalid_state(reason: &str) -> Self {
        Self::MiningInvalidState(reason.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_mining_timeout_error() {
        let error = AppError::mining_timeout(30);
        assert!(matches!(
            error,
            AppError::MiningTimeout {
                timeout_seconds: 30
            }
        ));

        let json_error = error.to_json_rpc_error(json!(1));
        assert_eq!(json_error["error"]["code"], -32007);
        assert!(json_error["error"]["message"]
            .as_str()
            .expect("Expected error message string")
            .contains("30 seconds"));
    }

    #[test]
    fn test_nonce_exhaustion_error() {
        let error = AppError::nonce_exhaustion(1000000);
        assert!(matches!(
            error,
            AppError::NonceExhaustion {
                nonces_tried: 1000000
            }
        ));

        let json_error = error.to_json_rpc_error(json!(1));
        assert_eq!(json_error["error"]["code"], -32008);
        assert!(json_error["error"]["message"]
            .as_str()
            .expect("Expected error message string")
            .contains("1000000 nonces"));
    }

    #[test]
    fn test_transaction_codec_error() {
        let error = AppError::transaction_codec_error("decode", "invalid binary format");

        if let AppError::TransactionCodecError { operation, reason } = error {
            assert_eq!(operation, "decode");
            assert_eq!(reason, "invalid binary format");
        } else {
            panic!("Expected TransactionCodecError");
        }

        let error = AppError::transaction_codec_error("decode", "invalid binary format");
        let json_error = error.to_json_rpc_error(json!(1));
        assert_eq!(json_error["error"]["code"], -32009);
        assert!(json_error["error"]["message"]
            .as_str()
            .expect("Expected error message string")
            .contains("decode"));
        assert!(json_error["error"]["message"]
            .as_str()
            .expect("Expected error message string")
            .contains("invalid binary format"));
    }

    #[test]
    fn test_mining_config_error() {
        let error = AppError::mining_config_error("invalid prefix length");

        let json_error = error.to_json_rpc_error(json!(1));
        assert_eq!(json_error["error"]["code"], -32010);
        assert!(json_error["error"]["message"]
            .as_str()
            .expect("Expected error message string")
            .contains("invalid prefix length"));
    }

    #[test]
    fn test_mining_invalid_state_error() {
        let error = AppError::mining_invalid_state("transaction already fully signed");

        let json_error = error.to_json_rpc_error(json!(1));
        assert_eq!(json_error["error"]["code"], -32011);
        assert!(json_error["error"]["message"]
            .as_str()
            .expect("Expected error message string")
            .contains("already fully signed"));
    }

    #[test]
    fn test_json_rpc_error_format() {
        let error = AppError::mining_timeout(10);
        let json_error = error.to_json_rpc_error(json!("test-id"));

        assert_eq!(json_error["jsonrpc"], "2.0");
        assert_eq!(json_error["id"], "test-id");
        assert!(json_error["error"]["code"].is_number());
        assert!(json_error["error"]["message"].is_string());
    }

    #[test]
    fn test_error_display() {
        let timeout_error = AppError::mining_timeout(15);
        assert_eq!(timeout_error.to_string(), "Mining timeout after 15 seconds");

        let nonce_error = AppError::nonce_exhaustion(500000);
        assert_eq!(
            nonce_error.to_string(),
            "Mining nonce exhaustion after trying 500000 nonces"
        );

        let codec_error = AppError::transaction_codec_error("encode", "serialization failed");
        assert_eq!(
            codec_error.to_string(),
            "Transaction codec error: encode failed - serialization failed"
        );
    }

    #[test]
    fn test_read_only_mode_error() {
        let error = AppError::ReadOnlyMode;
        assert!(matches!(error, AppError::ReadOnlyMode));

        let json_error = error.to_json_rpc_error(json!(1));
        assert_eq!(json_error["error"]["code"], -32000);
        assert_eq!(
            json_error["error"]["message"]
                .as_str()
                .expect("Error message should be a string"),
            "Read-only mode is enabled"
        );
    }
}
