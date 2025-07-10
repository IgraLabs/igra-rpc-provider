/// Wallet service domain-specific errors
#[derive(Debug, thiserror::Error)]
pub enum WalletError {
    /// Wallet connection failed
    #[error("Wallet connection failed: daemon_uri={daemon_uri}, reason={reason}")]
    ConnectionFailed { daemon_uri: String, reason: String },

    /// Wallet authentication failed
    #[error("Wallet authentication failed: {0}")]
    AuthenticationFailed(String),

    /// Wallet daemon unavailable
    #[error("Wallet daemon unavailable at {daemon_uri}")]
    DaemonUnavailable { daemon_uri: String },

    /// Transaction submission failed
    #[error("Transaction submission failed: {0}")]
    TransactionSubmissionFailed(String),

    /// Transaction signing failed
    #[error("Transaction signing failed: {0}")]
    TransactionSigningFailed(String),

    /// Insufficient wallet balance
    #[error("Insufficient wallet balance: required={required} SOMPI, available={available} SOMPI")]
    InsufficientBalance { required: u64, available: u64 },

    /// Invalid wallet address
    #[error("Invalid wallet address: {address}, reason={reason}")]
    InvalidAddress { address: String, reason: String },

    /// Invalid transaction amount
    #[error("Invalid transaction amount: {amount} SOMPI, reason={reason}")]
    InvalidAmount { amount: u64, reason: String },

    /// Invalid public key
    #[error("Invalid public key: {public_key}, reason={reason}")]
    InvalidPublicKey { public_key: String, reason: String },

    /// Wallet locked
    #[error("Wallet is locked, authentication required")]
    WalletLocked,

    /// Wallet not found
    #[error("Wallet not found: {wallet_id}")]
    WalletNotFound { wallet_id: String },

    /// Address generation failed
    #[error("Address generation failed: {0}")]
    AddressGenerationFailed(String),

    /// Mining operation failed
    #[error("Mining operation failed: {0}")]
    MiningFailed(String),

    /// Transaction broadcast failed
    #[error("Transaction broadcast failed: {0}")]
    BroadcastFailed(String),

    /// Transaction validation failed
    #[error("Transaction validation failed: {0}")]
    TransactionValidationFailed(String),

    /// UTXO selection failed
    #[error("UTXO selection failed: {0}")]
    UtxoSelectionFailed(String),

    /// Fee calculation failed
    #[error("Fee calculation failed: {0}")]
    FeeCalculationFailed(String),

    /// Network communication error
    #[error("Network communication error: {0}")]
    NetworkError(String),

    /// Wallet configuration error
    #[error("Wallet configuration error: {0}")]
    ConfigurationError(String),

    /// Wallet service timeout
    #[error("Wallet service timeout: operation={operation}, timeout_seconds={timeout_seconds}")]
    Timeout {
        operation: String,
        timeout_seconds: u64,
    },

    /// Concurrent operation error
    #[error("Concurrent wallet operation in progress: {operation}")]
    ConcurrentOperation { operation: String },

    /// Wallet service unavailable
    #[error("Wallet service temporarily unavailable")]
    ServiceUnavailable,

    /// Request validation failed
    #[error("Request validation failed: {0}")]
    RequestValidationFailed(String),

    /// Response parsing failed
    #[error("Response parsing failed: {0}")]
    ResponseParsingFailed(String),

    /// Protocol version mismatch
    #[error("Protocol version mismatch: expected={expected}, got={got}")]
    ProtocolVersionMismatch { expected: String, got: String },

    /// Internal wallet service error
    #[error("Internal wallet service error: {0}")]
    InternalError(String),
}

impl WalletError {
    /// Create a connection failed error
    pub fn connection_failed(daemon_uri: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::ConnectionFailed {
            daemon_uri: daemon_uri.into(),
            reason: reason.into(),
        }
    }

    /// Create a daemon unavailable error
    pub fn daemon_unavailable(daemon_uri: impl Into<String>) -> Self {
        Self::DaemonUnavailable {
            daemon_uri: daemon_uri.into(),
        }
    }

    /// Create an insufficient balance error
    pub fn insufficient_balance(required: u64, available: u64) -> Self {
        Self::InsufficientBalance {
            required,
            available,
        }
    }

    /// Create an invalid address error
    pub fn invalid_address(address: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidAddress {
            address: address.into(),
            reason: reason.into(),
        }
    }

    /// Create an invalid amount error
    pub fn invalid_amount(amount: u64, reason: impl Into<String>) -> Self {
        Self::InvalidAmount {
            amount,
            reason: reason.into(),
        }
    }

    /// Create an invalid public key error
    pub fn invalid_public_key(public_key: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidPublicKey {
            public_key: public_key.into(),
            reason: reason.into(),
        }
    }

    /// Create a wallet not found error
    pub fn wallet_not_found(wallet_id: impl Into<String>) -> Self {
        Self::WalletNotFound {
            wallet_id: wallet_id.into(),
        }
    }

    /// Create a timeout error
    pub fn timeout(operation: impl Into<String>, timeout_seconds: u64) -> Self {
        Self::Timeout {
            operation: operation.into(),
            timeout_seconds,
        }
    }

    /// Create a concurrent operation error
    pub fn concurrent_operation(operation: impl Into<String>) -> Self {
        Self::ConcurrentOperation {
            operation: operation.into(),
        }
    }

    /// Create a protocol version mismatch error
    pub fn protocol_version_mismatch(expected: impl Into<String>, got: impl Into<String>) -> Self {
        Self::ProtocolVersionMismatch {
            expected: expected.into(),
            got: got.into(),
        }
    }

    /// Check if this error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            WalletError::ConnectionFailed { .. }
                | WalletError::DaemonUnavailable { .. }
                | WalletError::NetworkError(_)
                | WalletError::Timeout { .. }
                | WalletError::ServiceUnavailable
                | WalletError::ConcurrentOperation { .. }
                | WalletError::InternalError(_)
        )
    }

    /// Check if this error is a validation error
    pub fn is_validation_error(&self) -> bool {
        matches!(
            self,
            WalletError::InvalidAddress { .. }
                | WalletError::InvalidAmount { .. }
                | WalletError::InvalidPublicKey { .. }
                | WalletError::TransactionValidationFailed(_)
                | WalletError::RequestValidationFailed(_)
        )
    }

    /// Check if this error is an authentication error
    pub fn is_authentication_error(&self) -> bool {
        matches!(
            self,
            WalletError::AuthenticationFailed(_) | WalletError::WalletLocked
        )
    }

    /// Check if this error is a configuration error
    pub fn is_configuration_error(&self) -> bool {
        matches!(self, WalletError::ConfigurationError(_))
    }

    /// Check if this error indicates insufficient resources
    pub fn is_resource_error(&self) -> bool {
        matches!(
            self,
            WalletError::InsufficientBalance { .. } | WalletError::UtxoSelectionFailed(_)
        )
    }

    /// Get error code for categorization
    pub fn error_code(&self) -> &'static str {
        match self {
            WalletError::ConnectionFailed { .. } => "CONNECTION_FAILED",
            WalletError::AuthenticationFailed(_) => "AUTHENTICATION_FAILED",
            WalletError::DaemonUnavailable { .. } => "DAEMON_UNAVAILABLE",
            WalletError::TransactionSubmissionFailed(_) => "TRANSACTION_SUBMISSION_FAILED",
            WalletError::TransactionSigningFailed(_) => "TRANSACTION_SIGNING_FAILED",
            WalletError::InsufficientBalance { .. } => "INSUFFICIENT_BALANCE",
            WalletError::InvalidAddress { .. } => "INVALID_ADDRESS",
            WalletError::InvalidAmount { .. } => "INVALID_AMOUNT",
            WalletError::InvalidPublicKey { .. } => "INVALID_PUBLIC_KEY",
            WalletError::WalletLocked => "WALLET_LOCKED",
            WalletError::WalletNotFound { .. } => "WALLET_NOT_FOUND",
            WalletError::AddressGenerationFailed(_) => "ADDRESS_GENERATION_FAILED",
            WalletError::MiningFailed(_) => "MINING_FAILED",
            WalletError::BroadcastFailed(_) => "BROADCAST_FAILED",
            WalletError::TransactionValidationFailed(_) => "TRANSACTION_VALIDATION_FAILED",
            WalletError::UtxoSelectionFailed(_) => "UTXO_SELECTION_FAILED",
            WalletError::FeeCalculationFailed(_) => "FEE_CALCULATION_FAILED",
            WalletError::NetworkError(_) => "NETWORK_ERROR",
            WalletError::ConfigurationError(_) => "CONFIGURATION_ERROR",
            WalletError::Timeout { .. } => "TIMEOUT",
            WalletError::ConcurrentOperation { .. } => "CONCURRENT_OPERATION",
            WalletError::ServiceUnavailable => "SERVICE_UNAVAILABLE",
            WalletError::RequestValidationFailed(_) => "REQUEST_VALIDATION_FAILED",
            WalletError::ResponseParsingFailed(_) => "RESPONSE_PARSING_FAILED",
            WalletError::ProtocolVersionMismatch { .. } => "PROTOCOL_VERSION_MISMATCH",
            WalletError::InternalError(_) => "INTERNAL_ERROR",
        }
    }

    /// Get suggested retry delay in seconds for retryable errors
    pub fn retry_delay_seconds(&self) -> Option<u64> {
        match self {
            WalletError::ConnectionFailed { .. } => Some(2),
            WalletError::DaemonUnavailable { .. } => Some(5),
            WalletError::NetworkError(_) => Some(1),
            WalletError::Timeout { .. } => Some(3),
            WalletError::ServiceUnavailable => Some(10),
            WalletError::ConcurrentOperation { .. } => Some(1),
            WalletError::InternalError(_) => Some(5),
            _ => None,
        }
    }

    /// Get the severity level of the error
    pub fn severity_level(&self) -> WalletErrorSeverity {
        match self {
            WalletError::InvalidAddress { .. }
            | WalletError::InvalidAmount { .. }
            | WalletError::InvalidPublicKey { .. }
            | WalletError::RequestValidationFailed(_) => WalletErrorSeverity::Low,

            WalletError::InsufficientBalance { .. }
            | WalletError::TransactionValidationFailed(_)
            | WalletError::UtxoSelectionFailed(_)
            | WalletError::FeeCalculationFailed(_)
            | WalletError::WalletLocked
            | WalletError::WalletNotFound { .. } => WalletErrorSeverity::Medium,

            WalletError::ConnectionFailed { .. }
            | WalletError::DaemonUnavailable { .. }
            | WalletError::ServiceUnavailable
            | WalletError::InternalError(_) => WalletErrorSeverity::High,

            WalletError::AuthenticationFailed(_)
            | WalletError::ConfigurationError(_)
            | WalletError::ProtocolVersionMismatch { .. } => WalletErrorSeverity::Critical,

            _ => WalletErrorSeverity::Medium,
        }
    }
}

/// Error severity levels for wallet operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalletErrorSeverity {
    /// Low severity - user input issues
    Low,
    /// Medium severity - operational issues
    Medium,
    /// High severity - service issues
    High,
    /// Critical severity - configuration or security issues
    Critical,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_failed_error() {
        let error = WalletError::connection_failed("http://localhost:8082", "Connection refused");
        assert!(error.is_retryable());
        assert!(!error.is_validation_error());
        assert_eq!(error.error_code(), "CONNECTION_FAILED");
        assert_eq!(error.retry_delay_seconds(), Some(2));
        assert_eq!(error.severity_level(), WalletErrorSeverity::High);
    }

    #[test]
    fn test_insufficient_balance_error() {
        let error = WalletError::insufficient_balance(1000, 500);
        assert!(!error.is_retryable());
        assert!(!error.is_validation_error());
        assert!(error.is_resource_error());
        assert_eq!(error.error_code(), "INSUFFICIENT_BALANCE");
        assert_eq!(error.severity_level(), WalletErrorSeverity::Medium);
    }

    #[test]
    fn test_invalid_address_error() {
        let error = WalletError::invalid_address("invalid", "Wrong format");
        assert!(!error.is_retryable());
        assert!(error.is_validation_error());
        assert!(!error.is_resource_error());
        assert_eq!(error.error_code(), "INVALID_ADDRESS");
        assert_eq!(error.severity_level(), WalletErrorSeverity::Low);
    }

    #[test]
    fn test_authentication_failed_error() {
        let error = WalletError::AuthenticationFailed("Invalid password".to_string());
        assert!(!error.is_retryable());
        assert!(error.is_authentication_error());
        assert_eq!(error.error_code(), "AUTHENTICATION_FAILED");
        assert_eq!(error.severity_level(), WalletErrorSeverity::Critical);
    }

    #[test]
    fn test_wallet_locked_error() {
        let error = WalletError::WalletLocked;
        assert!(!error.is_retryable());
        assert!(error.is_authentication_error());
        assert_eq!(error.error_code(), "WALLET_LOCKED");
        assert_eq!(error.severity_level(), WalletErrorSeverity::Medium);
    }

    #[test]
    fn test_timeout_error() {
        let error = WalletError::timeout("transaction_signing", 30);
        assert!(error.is_retryable());
        assert!(!error.is_validation_error());
        assert_eq!(error.error_code(), "TIMEOUT");
        assert_eq!(error.retry_delay_seconds(), Some(3));
        assert_eq!(error.severity_level(), WalletErrorSeverity::Medium);
    }

    #[test]
    fn test_concurrent_operation_error() {
        let error = WalletError::concurrent_operation("transaction_signing");
        assert!(error.is_retryable());
        assert!(!error.is_validation_error());
        assert_eq!(error.error_code(), "CONCURRENT_OPERATION");
        assert_eq!(error.retry_delay_seconds(), Some(1));
    }

    #[test]
    fn test_protocol_version_mismatch_error() {
        let error = WalletError::protocol_version_mismatch("1.0", "2.0");
        assert!(!error.is_retryable());
        assert!(!error.is_validation_error());
        assert_eq!(error.error_code(), "PROTOCOL_VERSION_MISMATCH");
        assert_eq!(error.severity_level(), WalletErrorSeverity::Critical);
    }
}
