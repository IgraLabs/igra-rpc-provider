use serde_json::{json, Value};
use thiserror::Error;

#[cfg(test)]
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
}

// Conversion from KaspaWalletError to AppError is only reachable from
// the test-only proto/kaspa round-trip converters in `types::wallet`;
// it's gated to keep the production error surface honest.
#[cfg(test)]
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
    /// `id` is the JSON-RPC request id used to correlate the error with the
    /// original request.
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_json_rpc_error_format() {
        let error = AppError::RetryExhausted {
            attempts: 5,
            reason: "test".to_string(),
        };
        let json_error = error.to_json_rpc_error(json!("test-id"));

        assert_eq!(json_error["jsonrpc"], "2.0");
        assert_eq!(json_error["id"], "test-id");
        assert!(json_error["error"]["code"].is_number());
        assert!(json_error["error"]["message"].is_string());
    }

    #[test]
    fn test_error_display() {
        let retry_error = AppError::RetryExhausted {
            attempts: 3,
            reason: "UTXO exhaustion".to_string(),
        };
        assert_eq!(
            retry_error.to_string(),
            "Retry exhausted after 3 attempts: UTXO exhaustion"
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
