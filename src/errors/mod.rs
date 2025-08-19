/// Domain-specific error types with single responsibility
///
/// This module contains error types organized by domain, following the single
/// responsibility principle. Each error type is specific to its domain and
/// provides detailed context for error handling and debugging.
pub mod gas;
pub mod proxy;
pub mod transaction;
pub mod wallet;

// Re-export all error types for easier access
pub use gas::GasError;
pub use proxy::ProxyError;
pub use transaction::TransactionError;
pub use wallet::{WalletError, WalletErrorSeverity};

/// Convert domain errors to JSON-RPC error format
pub trait ToJsonRpcError {
    /// Convert the error to a JSON-RPC error response
    fn to_json_rpc_error(&self, id: serde_json::Value) -> serde_json::Value;

    /// Get the JSON-RPC error code for this error
    fn json_rpc_error_code(&self) -> i32;
}

impl ToJsonRpcError for TransactionError {
    fn to_json_rpc_error(&self, id: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "jsonrpc": "2.0",
            "error": {
                "code": self.json_rpc_error_code(),
                "message": self.to_string(),
                "data": {
                    "error_code": self.error_code(),
                    "retryable": self.is_retryable(),
                    "validation_error": self.is_validation_error()
                }
            },
            "id": id
        })
    }

    fn json_rpc_error_code(&self) -> i32 {
        match self {
            TransactionError::DecodingFailed(_) => -32700, // Parse error
            TransactionError::ValidationFailed(_) => -32602, // Invalid params
            TransactionError::GasPriceValidationFailed { .. } => -32602, // Invalid params
            TransactionError::Eip1559ValidationFailed { .. } => -32602, // Invalid params
            TransactionError::LegacyGasPriceTooLow { .. } => -32602, // Invalid params
            TransactionError::MissingGasPricing => -32602, // Invalid params
            TransactionError::PayloadValidationFailed(_) => -32602, // Invalid params
            TransactionError::TransactionTooLarge { .. } => -32602, // Invalid params
            TransactionError::InvalidFormat(_) => -32602,  // Invalid params
            TransactionError::InvalidNonce { .. } => -32602, // Invalid params
            TransactionError::InsufficientFunds { .. } => -32602, // Invalid params
            TransactionError::SignatureVerificationFailed(_) => -32602, // Invalid params
            TransactionError::QueueFull { .. } => -32000,  // Server error
            TransactionError::ProcessingTimeout { .. } => -32000, // Server error
            TransactionError::MiningFailed(_) => -32000,   // Server error
            TransactionError::WalletOperationFailed(_) => -32000, // Server error
            TransactionError::AlreadyExists { .. } => -32000, // Server error
            TransactionError::TransactionExpired { .. } => -32000, // Server error
            TransactionError::InternalError(_) => -32603,  // Internal error
            TransactionError::InvalidTransactionFormat(_) => -32602, // Invalid params
            TransactionError::InsufficientGasFee { .. } => -32602, // Invalid params
        }
    }
}

impl ToJsonRpcError for GasError {
    fn to_json_rpc_error(&self, id: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "jsonrpc": "2.0",
            "error": {
                "code": self.json_rpc_error_code(),
                "message": self.to_string(),
                "data": {
                    "error_code": self.error_code(),
                    "retryable": self.is_retryable(),
                    "validation_error": self.is_validation_error()
                }
            },
            "id": id
        })
    }

    fn json_rpc_error_code(&self) -> i32 {
        match self {
            GasError::InvalidFormat { .. } => -32602, // Invalid params
            GasError::Eip1559ValidationFailed { .. } => -32602, // Invalid params
            GasError::LegacyValidationFailed { .. } => -32602, // Invalid params
            GasError::BelowMinimumFloor { .. } => -32602, // Invalid params
            GasError::PriorityFeeExceedsMax { .. } => -32602, // Invalid params
            GasError::MaxFeeBelowBase { .. } => -32602, // Invalid params
            GasError::InvalidConfiguration { .. } => -32602, // Invalid params
            GasError::ConversionError(_) => -32700,   // Parse error
            GasError::CalculationFailed(_) => -32000, // Server error
            GasError::BaseFetchFailed(_) => -32000,   // Server error
            GasError::CacheError(_) => -32000,        // Server error
            GasError::ArithmeticOverflow { .. } => -32000, // Server error
            GasError::ConfigurationError(_) => -32000, // Server error
            GasError::NetworkError(_) => -32000,      // Server error
            GasError::ResponseParsingError(_) => -32000, // Server error
            GasError::ServiceUnavailable => -32000,   // Server error
            GasError::StaleData { .. } => -32000,     // Server error
            GasError::InternalError(_) => -32603,     // Internal error
        }
    }
}

impl ToJsonRpcError for ProxyError {
    fn to_json_rpc_error(&self, id: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "jsonrpc": "2.0",
            "error": {
                "code": self.json_rpc_error_code(),
                "message": self.to_string(),
                "data": {
                    "error_code": self.error_code(),
                    "retryable": self.is_retryable(),
                    "http_status": self.http_status_code()
                }
            },
            "id": id
        })
    }

    fn json_rpc_error_code(&self) -> i32 {
        match self {
            ProxyError::SerializationFailed(_) => -32700, // Parse error
            ProxyError::DeserializationFailed(_) => -32700, // Parse error
            ProxyError::InvalidRequestFormat(_) => -32600, // Invalid request
            ProxyError::InvalidResponseFormat(_) => -32700, // Parse error
            ProxyError::UnsupportedMethod { .. } => -32601, // Method not found
            ProxyError::RequestValidationFailed(_) => -32602, // Invalid params
            ProxyError::RequestTooLarge { .. } => -32602, // Invalid params
            ProxyError::RateLimitExceeded { .. } => -32000, // Server error
            ProxyError::ElCommunicationFailed(_) => -32000, // Server error
            ProxyError::ElConnectionFailed { .. } => -32000, // Server error
            ProxyError::ElTimeout { .. } => -32000,       // Server error
            ProxyError::ElClientError { .. } => -32000,   // Server error
            ProxyError::RoutingFailed { .. } => -32000,   // Server error
            ProxyError::ResponseTransformationFailed(_) => -32000, // Server error
            ProxyError::ServiceUnavailable => -32000,     // Server error
            ProxyError::ResponseTooLarge { .. } => -32000, // Server error
            ProxyError::CircuitBreakerOpen { .. } => -32000, // Server error
            ProxyError::LoadBalancerError(_) => -32000,   // Server error
            ProxyError::InvalidElUrl { .. } => -32000,    // Server error
            ProxyError::ConfigurationError(_) => -32000,  // Server error
            ProxyError::InternalError(_) => -32603,       // Internal error
        }
    }
}

impl ToJsonRpcError for WalletError {
    fn to_json_rpc_error(&self, id: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "jsonrpc": "2.0",
            "error": {
                "code": self.json_rpc_error_code(),
                "message": self.to_string(),
                "data": {
                    "error_code": self.error_code(),
                    "retryable": self.is_retryable(),
                    "validation_error": self.is_validation_error(),
                    "severity": format!("{:?}", self.severity_level())
                }
            },
            "id": id
        })
    }

    fn json_rpc_error_code(&self) -> i32 {
        match self {
            WalletError::InvalidAddress { .. } => -32602, // Invalid params
            WalletError::InvalidAmount { .. } => -32602,  // Invalid params
            WalletError::InvalidPublicKey { .. } => -32602, // Invalid params
            WalletError::RequestValidationFailed(_) => -32602, // Invalid params
            WalletError::TransactionValidationFailed(_) => -32602, // Invalid params
            WalletError::ResponseParsingFailed(_) => -32700, // Parse error
            WalletError::AuthenticationFailed(_) => -32000, // Server error
            WalletError::WalletLocked => -32000,          // Server error
            WalletError::InsufficientBalance { .. } => -32000, // Server error
            WalletError::ConnectionFailed { .. } => -32000, // Server error
            WalletError::DaemonUnavailable { .. } => -32000, // Server error
            WalletError::TransactionSubmissionFailed(_) => -32000, // Server error
            WalletError::TransactionSigningFailed(_) => -32000, // Server error
            WalletError::WalletNotFound { .. } => -32000, // Server error
            WalletError::AddressGenerationFailed(_) => -32000, // Server error
            WalletError::MiningFailed(_) => -32000,       // Server error
            WalletError::BroadcastFailed(_) => -32000,    // Server error
            WalletError::UtxoSelectionFailed(_) => -32000, // Server error
            WalletError::FeeCalculationFailed(_) => -32000, // Server error
            WalletError::NetworkError(_) => -32000,       // Server error
            WalletError::ConfigurationError(_) => -32000, // Server error
            WalletError::Timeout { .. } => -32000,        // Server error
            WalletError::ConcurrentOperation { .. } => -32000, // Server error
            WalletError::ServiceUnavailable => -32000,    // Server error
            WalletError::ProtocolVersionMismatch { .. } => -32000, // Server error
            WalletError::InternalError(_) => -32603,      // Internal error
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_transaction_error_to_json_rpc() {
        let error = TransactionError::gas_price_validation_failed("Price too low");
        let json_error = error.to_json_rpc_error(json!(1));

        assert_eq!(json_error["jsonrpc"], "2.0");
        assert_eq!(json_error["id"], 1);
        assert_eq!(json_error["error"]["code"], -32602);
        assert!(json_error["error"]["message"]
            .as_str()
            .expect("message should be a string")
            .contains("Price too low"));
        assert_eq!(
            json_error["error"]["data"]["error_code"],
            "GAS_PRICE_VALIDATION_FAILED"
        );
        assert_eq!(json_error["error"]["data"]["retryable"], false);
        assert_eq!(json_error["error"]["data"]["validation_error"], true);
    }

    #[test]
    fn test_gas_error_to_json_rpc() {
        let error = GasError::base_fetch_failed("Network timeout");
        let json_error = error.to_json_rpc_error(json!(2));

        assert_eq!(json_error["jsonrpc"], "2.0");
        assert_eq!(json_error["id"], 2);
        assert_eq!(json_error["error"]["code"], -32000);
        assert_eq!(
            json_error["error"]["data"]["error_code"],
            "BASE_FETCH_FAILED"
        );
        assert_eq!(json_error["error"]["data"]["retryable"], true);
    }

    #[test]
    fn test_proxy_error_to_json_rpc() {
        let error = ProxyError::unsupported_method("eth_unsupportedMethod");
        let json_error = error.to_json_rpc_error(json!(3));

        assert_eq!(json_error["jsonrpc"], "2.0");
        assert_eq!(json_error["id"], 3);
        assert_eq!(json_error["error"]["code"], -32601);
        assert_eq!(
            json_error["error"]["data"]["error_code"],
            "UNSUPPORTED_METHOD"
        );
        assert_eq!(json_error["error"]["data"]["http_status"], 400);
    }

    #[test]
    fn test_wallet_error_to_json_rpc() {
        let error = WalletError::insufficient_balance(1000, 500);
        let json_error = error.to_json_rpc_error(json!(4));

        assert_eq!(json_error["jsonrpc"], "2.0");
        assert_eq!(json_error["id"], 4);
        assert_eq!(json_error["error"]["code"], -32000);
        assert_eq!(
            json_error["error"]["data"]["error_code"],
            "INSUFFICIENT_BALANCE"
        );
        assert_eq!(json_error["error"]["data"]["severity"], "Medium");
    }
}
