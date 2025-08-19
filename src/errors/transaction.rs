/// Transaction-specific errors for the transaction processing domain
#[derive(Debug, thiserror::Error)]
pub enum TransactionError {
    /// Transaction decoding failed
    #[error("Failed to decode transaction: {0}")]
    DecodingFailed(String),

    /// Transaction validation failed
    #[error("Transaction validation failed: {0}")]
    ValidationFailed(String),

    /// Gas price validation failed
    #[error("Gas price validation failed: {reason}")]
    GasPriceValidationFailed { reason: String },

    /// EIP-1559 fee validation failed
    #[error("EIP-1559 fee validation failed: max_fee={max_fee}, priority_fee={priority_fee}, base_fee={base_fee}")]
    Eip1559ValidationFailed {
        max_fee: String,
        priority_fee: String,
        base_fee: String,
    },

    /// Legacy gas price too low
    #[error("Legacy gas price too low: gas_price={gas_price}, required_base_fee={base_fee}")]
    LegacyGasPriceTooLow { gas_price: String, base_fee: String },

    /// Transaction missing required gas pricing information
    #[error("Transaction missing gas pricing information")]
    MissingGasPricing,

    /// Transaction payload validation failed
    #[error("Transaction payload validation failed: {0}")]
    PayloadValidationFailed(String),

    /// Transaction too large
    #[error("Transaction too large: size={size} bytes, max_allowed={max_allowed} bytes")]
    TransactionTooLarge { size: usize, max_allowed: usize },

    /// Transaction expired
    #[error(
        "Transaction expired: current_block={current_block}, transaction_block={transaction_block}"
    )]
    TransactionExpired {
        current_block: u64,
        transaction_block: u64,
    },

    /// Nonce validation failed
    #[error("Nonce validation failed: expected={expected}, got={got}")]
    InvalidNonce { expected: u64, got: u64 },

    /// Transaction queue full
    #[error("Transaction queue is full, capacity={capacity}")]
    QueueFull { capacity: usize },

    /// Transaction processing timeout
    #[error("Transaction processing timed out after {timeout_seconds} seconds")]
    ProcessingTimeout { timeout_seconds: u64 },

    /// Mining operation failed
    #[error("Transaction mining failed: {0}")]
    MiningFailed(String),

    /// Wallet operation failed
    #[error("Wallet operation failed: {0}")]
    WalletOperationFailed(String),

    /// Invalid transaction format
    #[error("Invalid transaction format: {0}")]
    InvalidFormat(String),

    /// Transaction already exists
    #[error("Transaction already exists: hash={hash}")]
    AlreadyExists { hash: String },

    /// Insufficient funds for transaction
    #[error("Insufficient funds: required={required}, available={available}")]
    InsufficientFunds { required: String, available: String },

    /// Transaction signature verification failed
    #[error("Transaction signature verification failed: {0}")]
    SignatureVerificationFailed(String),

    /// Internal processing error
    #[error("Internal transaction processing error: {0}")]
    InternalError(String),

    /// Invalid transaction format (missing fields or unsupported types)
    #[error("Invalid transaction format: {0}")]
    InvalidTransactionFormat(String),

    /// Insufficient gas fee for protocol requirements
    #[error("Insufficient gas fee: required {required} wei, provided {provided} wei")]
    InsufficientGasFee { required: String, provided: String },
}

impl TransactionError {
    /// Create a gas price validation error with context
    pub fn gas_price_validation_failed(reason: impl Into<String>) -> Self {
        Self::GasPriceValidationFailed {
            reason: reason.into(),
        }
    }

    /// Create an EIP-1559 validation error with fee details
    pub fn eip1559_validation_failed(
        max_fee: impl Into<String>,
        priority_fee: impl Into<String>,
        base_fee: impl Into<String>,
    ) -> Self {
        Self::Eip1559ValidationFailed {
            max_fee: max_fee.into(),
            priority_fee: priority_fee.into(),
            base_fee: base_fee.into(),
        }
    }

    /// Create a legacy gas price error with pricing details
    pub fn legacy_gas_price_too_low(
        gas_price: impl Into<String>,
        base_fee: impl Into<String>,
    ) -> Self {
        Self::LegacyGasPriceTooLow {
            gas_price: gas_price.into(),
            base_fee: base_fee.into(),
        }
    }

    /// Create a transaction size error
    pub fn transaction_too_large(size: usize, max_allowed: usize) -> Self {
        Self::TransactionTooLarge { size, max_allowed }
    }

    /// Create a transaction expiration error
    pub fn transaction_expired(current_block: u64, transaction_block: u64) -> Self {
        Self::TransactionExpired {
            current_block,
            transaction_block,
        }
    }

    /// Create an invalid nonce error
    pub fn invalid_nonce(expected: u64, got: u64) -> Self {
        Self::InvalidNonce { expected, got }
    }

    /// Create a queue full error
    pub fn queue_full(capacity: usize) -> Self {
        Self::QueueFull { capacity }
    }

    /// Create a processing timeout error
    pub fn processing_timeout(timeout_seconds: u64) -> Self {
        Self::ProcessingTimeout { timeout_seconds }
    }

    /// Create an already exists error
    pub fn already_exists(hash: impl Into<String>) -> Self {
        Self::AlreadyExists { hash: hash.into() }
    }

    /// Create an insufficient funds error
    pub fn insufficient_funds(required: impl Into<String>, available: impl Into<String>) -> Self {
        Self::InsufficientFunds {
            required: required.into(),
            available: available.into(),
        }
    }

    /// Create an invalid transaction format error
    pub fn invalid_transaction_format(reason: impl Into<String>) -> Self {
        Self::InvalidTransactionFormat(reason.into())
    }

    /// Create an insufficient gas fee error
    pub fn insufficient_gas_fee(required: impl Into<String>, provided: impl Into<String>) -> Self {
        Self::InsufficientGasFee {
            required: required.into(),
            provided: provided.into(),
        }
    }

    /// Create a decoding failed error
    pub fn decoding_failed(reason: impl Into<String>) -> Self {
        Self::DecodingFailed(reason.into())
    }

    /// Create a validation failed error  
    pub fn validation_failed(reason: impl Into<String>) -> Self {
        Self::ValidationFailed(reason.into())
    }

    /// Check if this error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            TransactionError::QueueFull { .. }
                | TransactionError::ProcessingTimeout { .. }
                | TransactionError::InternalError(_)
                | TransactionError::MiningFailed(_)
                | TransactionError::WalletOperationFailed(_)
        )
    }

    /// Check if this error is a validation error
    pub fn is_validation_error(&self) -> bool {
        matches!(
            self,
            TransactionError::ValidationFailed(_)
                | TransactionError::GasPriceValidationFailed { .. }
                | TransactionError::Eip1559ValidationFailed { .. }
                | TransactionError::LegacyGasPriceTooLow { .. }
                | TransactionError::MissingGasPricing
                | TransactionError::PayloadValidationFailed(_)
                | TransactionError::InvalidNonce { .. }
                | TransactionError::TransactionTooLarge { .. }
                | TransactionError::TransactionExpired { .. }
                | TransactionError::InvalidFormat(_)
                | TransactionError::InsufficientFunds { .. }
                | TransactionError::SignatureVerificationFailed(_)
                | TransactionError::InvalidTransactionFormat(_)
                | TransactionError::InsufficientGasFee { .. }
                | TransactionError::DecodingFailed(_)
        )
    }

    /// Get error code for categorization
    pub fn error_code(&self) -> &'static str {
        match self {
            TransactionError::DecodingFailed(_) => "DECODE_FAILED",
            TransactionError::ValidationFailed(_) => "VALIDATION_FAILED",
            TransactionError::GasPriceValidationFailed { .. } => "GAS_PRICE_VALIDATION_FAILED",
            TransactionError::Eip1559ValidationFailed { .. } => "EIP1559_VALIDATION_FAILED",
            TransactionError::LegacyGasPriceTooLow { .. } => "LEGACY_GAS_PRICE_TOO_LOW",
            TransactionError::MissingGasPricing => "MISSING_GAS_PRICING",
            TransactionError::PayloadValidationFailed(_) => "PAYLOAD_VALIDATION_FAILED",
            TransactionError::TransactionTooLarge { .. } => "TRANSACTION_TOO_LARGE",
            TransactionError::TransactionExpired { .. } => "TRANSACTION_EXPIRED",
            TransactionError::InvalidNonce { .. } => "INVALID_NONCE",
            TransactionError::QueueFull { .. } => "QUEUE_FULL",
            TransactionError::ProcessingTimeout { .. } => "PROCESSING_TIMEOUT",
            TransactionError::MiningFailed(_) => "MINING_FAILED",
            TransactionError::WalletOperationFailed(_) => "WALLET_OPERATION_FAILED",
            TransactionError::InvalidFormat(_) => "INVALID_FORMAT",
            TransactionError::AlreadyExists { .. } => "ALREADY_EXISTS",
            TransactionError::InsufficientFunds { .. } => "INSUFFICIENT_FUNDS",
            TransactionError::SignatureVerificationFailed(_) => "SIGNATURE_VERIFICATION_FAILED",
            TransactionError::InternalError(_) => "INTERNAL_ERROR",
            TransactionError::InvalidTransactionFormat(_) => "INVALID_TRANSACTION_FORMAT",
            TransactionError::InsufficientGasFee { .. } => "INSUFFICIENT_GAS_FEE",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gas_price_validation_error() {
        let error = TransactionError::gas_price_validation_failed("Price too low");
        assert!(error.is_validation_error());
        assert!(!error.is_retryable());
        assert_eq!(error.error_code(), "GAS_PRICE_VALIDATION_FAILED");
    }

    #[test]
    fn test_eip1559_validation_error() {
        let error = TransactionError::eip1559_validation_failed("100", "10", "95");
        assert!(error.is_validation_error());
        assert!(!error.is_retryable());
        assert_eq!(error.error_code(), "EIP1559_VALIDATION_FAILED");
    }

    #[test]
    fn test_queue_full_error() {
        let error = TransactionError::queue_full(1000);
        assert!(!error.is_validation_error());
        assert!(error.is_retryable());
        assert_eq!(error.error_code(), "QUEUE_FULL");
    }

    #[test]
    fn test_transaction_size_error() {
        let error = TransactionError::transaction_too_large(1500, 1000);
        assert!(error.is_validation_error());
        assert!(!error.is_retryable());
        assert_eq!(error.error_code(), "TRANSACTION_TOO_LARGE");
    }

    #[test]
    fn test_insufficient_funds_error() {
        let error = TransactionError::insufficient_funds("1000", "500");
        assert!(error.is_validation_error());
        assert!(!error.is_retryable());
        assert_eq!(error.error_code(), "INSUFFICIENT_FUNDS");
    }

    #[test]
    fn test_invalid_transaction_format_error() {
        let error =
            TransactionError::invalid_transaction_format("Missing max_priority_fee_per_gas");
        assert!(error.is_validation_error());
        assert!(!error.is_retryable());
        assert_eq!(error.error_code(), "INVALID_TRANSACTION_FORMAT");
        assert!(error.to_string().contains("Invalid transaction format"));
    }

    #[test]
    fn test_insufficient_gas_fee_error() {
        let error = TransactionError::insufficient_gas_fee("2000000000000", "1000000000");
        assert!(error.is_validation_error());
        assert!(!error.is_retryable());
        assert_eq!(error.error_code(), "INSUFFICIENT_GAS_FEE");
        assert!(error.to_string().contains("required 2000000000000 wei"));
        assert!(error.to_string().contains("provided 1000000000 wei"));
    }
}
