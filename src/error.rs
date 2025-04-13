use serde_json::{json, Value};
use thiserror::Error;

/// Custom application error type for handling various types of errors.
#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum AppError {
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
            AppError::InvalidTransactionFormat => {
                (-32001, "Invalid L2 transaction format".to_string())
            }
            AppError::ElCallError(_) => (-32000, "IGRA EL Client call failed".to_string()),
            AppError::WalletCallError => (-32005, "KASPA Wallet call failed".to_string()),
            AppError::MethodNotAllowed(method) => {
                (-32002, format!("RPC method not allowed: {}", method))
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
}
