use thiserror::Error;
use serde_json::{json, Value};

/// Custom application error type for handling various types of errors.
#[derive(Debug, Error)]
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
            AppError::InvalidTransactionFormat => (-32001, "Invalid L2 transaction format"),
            AppError::ElCallError(_) => (-32000, "IGRA EL Client call failed"),
            AppError::WalletCallError => (-32005, "KASPA Wallet call failed"),
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
