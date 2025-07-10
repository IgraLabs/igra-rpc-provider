/// Gas pricing domain-specific errors
#[derive(Debug, thiserror::Error)]
pub enum GasError {
    /// Gas price calculation failed
    #[error("Gas price calculation failed: {0}")]
    CalculationFailed(String),

    /// Failed to fetch base fee from EL client
    #[error("Failed to fetch base fee from EL client: {0}")]
    BaseFetchFailed(String),

    /// Invalid gas price format
    #[error("Invalid gas price format: {value}, expected hex string")]
    InvalidFormat { value: String },

    /// Gas price conversion error
    #[error("Gas price conversion error: {0}")]
    ConversionError(String),

    /// EIP-1559 fee validation failed
    #[error("EIP-1559 fee validation failed: {reason}")]
    Eip1559ValidationFailed { reason: String },

    /// Legacy gas price validation failed
    #[error("Legacy gas price validation failed: {reason}")]
    LegacyValidationFailed { reason: String },

    /// Gas price below minimum floor
    #[error("Gas price below minimum floor: price={price} wei, floor={floor} wei")]
    BelowMinimumFloor { price: String, floor: String },

    /// Priority fee exceeds max fee in EIP-1559 transaction
    #[error("Priority fee exceeds max fee: priority_fee={priority_fee}, max_fee={max_fee}")]
    PriorityFeeExceedsMax {
        priority_fee: String,
        max_fee: String,
    },

    /// Max fee below current base fee
    #[error("Max fee below current base fee: max_fee={max_fee}, base_fee={base_fee}")]
    MaxFeeBelowBase { max_fee: String, base_fee: String },

    /// Gas price cache error
    #[error("Gas price cache error: {0}")]
    CacheError(String),

    /// Gas price arithmetic overflow
    #[error("Gas price arithmetic overflow: {operation}")]
    ArithmeticOverflow { operation: String },

    /// Configuration error
    #[error("Gas configuration error: {0}")]
    ConfigurationError(String),

    /// Network communication error when fetching gas prices
    #[error("Network error fetching gas prices: {0}")]
    NetworkError(String),

    /// Gas price response parsing error
    #[error("Failed to parse gas price response: {0}")]
    ResponseParsingError(String),

    /// Gas price service unavailable
    #[error("Gas price service temporarily unavailable")]
    ServiceUnavailable,

    /// Invalid gas configuration
    #[error("Invalid gas configuration: {field}={value}, {reason}")]
    InvalidConfiguration {
        field: String,
        value: String,
        reason: String,
    },

    /// Gas price stale (cache expired beyond acceptable threshold)
    #[error("Gas price data is stale: age={age_seconds}s, max_age={max_age_seconds}s")]
    StaleData {
        age_seconds: u64,
        max_age_seconds: u64,
    },

    /// Internal gas service error
    #[error("Internal gas service error: {0}")]
    InternalError(String),
}

impl GasError {
    /// Create a base fee fetch error
    pub fn base_fetch_failed(reason: impl Into<String>) -> Self {
        Self::BaseFetchFailed(reason.into())
    }

    /// Create an invalid format error
    pub fn invalid_format(value: impl Into<String>) -> Self {
        Self::InvalidFormat {
            value: value.into(),
        }
    }

    /// Create an EIP-1559 validation error
    pub fn eip1559_validation_failed(reason: impl Into<String>) -> Self {
        Self::Eip1559ValidationFailed {
            reason: reason.into(),
        }
    }

    /// Create a legacy validation error
    pub fn legacy_validation_failed(reason: impl Into<String>) -> Self {
        Self::LegacyValidationFailed {
            reason: reason.into(),
        }
    }

    /// Create a below minimum floor error
    pub fn below_minimum_floor(price: impl Into<String>, floor: impl Into<String>) -> Self {
        Self::BelowMinimumFloor {
            price: price.into(),
            floor: floor.into(),
        }
    }

    /// Create a priority fee exceeds max error
    pub fn priority_fee_exceeds_max(
        priority_fee: impl Into<String>,
        max_fee: impl Into<String>,
    ) -> Self {
        Self::PriorityFeeExceedsMax {
            priority_fee: priority_fee.into(),
            max_fee: max_fee.into(),
        }
    }

    /// Create a max fee below base error
    pub fn max_fee_below_base(max_fee: impl Into<String>, base_fee: impl Into<String>) -> Self {
        Self::MaxFeeBelowBase {
            max_fee: max_fee.into(),
            base_fee: base_fee.into(),
        }
    }

    /// Create an arithmetic overflow error
    pub fn arithmetic_overflow(operation: impl Into<String>) -> Self {
        Self::ArithmeticOverflow {
            operation: operation.into(),
        }
    }

    /// Create an invalid configuration error
    pub fn invalid_configuration(
        field: impl Into<String>,
        value: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self::InvalidConfiguration {
            field: field.into(),
            value: value.into(),
            reason: reason.into(),
        }
    }

    /// Create a stale data error
    pub fn stale_data(age_seconds: u64, max_age_seconds: u64) -> Self {
        Self::StaleData {
            age_seconds,
            max_age_seconds,
        }
    }

    /// Check if this error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            GasError::BaseFetchFailed(_)
                | GasError::NetworkError(_)
                | GasError::ServiceUnavailable
                | GasError::CacheError(_)
                | GasError::ResponseParsingError(_)
                | GasError::InternalError(_)
        )
    }

    /// Check if this error is a validation error
    pub fn is_validation_error(&self) -> bool {
        matches!(
            self,
            GasError::Eip1559ValidationFailed { .. }
                | GasError::LegacyValidationFailed { .. }
                | GasError::BelowMinimumFloor { .. }
                | GasError::PriorityFeeExceedsMax { .. }
                | GasError::MaxFeeBelowBase { .. }
                | GasError::InvalidFormat { .. }
                | GasError::InvalidConfiguration { .. }
        )
    }

    /// Check if this error is a configuration error
    pub fn is_configuration_error(&self) -> bool {
        matches!(
            self,
            GasError::ConfigurationError(_) | GasError::InvalidConfiguration { .. }
        )
    }

    /// Get error code for categorization
    pub fn error_code(&self) -> &'static str {
        match self {
            GasError::CalculationFailed(_) => "CALCULATION_FAILED",
            GasError::BaseFetchFailed(_) => "BASE_FETCH_FAILED",
            GasError::InvalidFormat { .. } => "INVALID_FORMAT",
            GasError::ConversionError(_) => "CONVERSION_ERROR",
            GasError::Eip1559ValidationFailed { .. } => "EIP1559_VALIDATION_FAILED",
            GasError::LegacyValidationFailed { .. } => "LEGACY_VALIDATION_FAILED",
            GasError::BelowMinimumFloor { .. } => "BELOW_MINIMUM_FLOOR",
            GasError::PriorityFeeExceedsMax { .. } => "PRIORITY_FEE_EXCEEDS_MAX",
            GasError::MaxFeeBelowBase { .. } => "MAX_FEE_BELOW_BASE",
            GasError::CacheError(_) => "CACHE_ERROR",
            GasError::ArithmeticOverflow { .. } => "ARITHMETIC_OVERFLOW",
            GasError::ConfigurationError(_) => "CONFIGURATION_ERROR",
            GasError::NetworkError(_) => "NETWORK_ERROR",
            GasError::ResponseParsingError(_) => "RESPONSE_PARSING_ERROR",
            GasError::ServiceUnavailable => "SERVICE_UNAVAILABLE",
            GasError::InvalidConfiguration { .. } => "INVALID_CONFIGURATION",
            GasError::StaleData { .. } => "STALE_DATA",
            GasError::InternalError(_) => "INTERNAL_ERROR",
        }
    }

    /// Get suggested retry delay in seconds for retryable errors
    pub fn retry_delay_seconds(&self) -> Option<u64> {
        match self {
            GasError::BaseFetchFailed(_) => Some(1),
            GasError::NetworkError(_) => Some(2),
            GasError::ServiceUnavailable => Some(5),
            GasError::CacheError(_) => Some(1),
            GasError::ResponseParsingError(_) => Some(1),
            GasError::InternalError(_) => Some(3),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eip1559_validation_error() {
        let error = GasError::eip1559_validation_failed("Priority fee too high");
        assert!(error.is_validation_error());
        assert!(!error.is_retryable());
        assert_eq!(error.error_code(), "EIP1559_VALIDATION_FAILED");
    }

    #[test]
    fn test_base_fetch_failed_error() {
        let error = GasError::base_fetch_failed("Network timeout");
        assert!(!error.is_validation_error());
        assert!(error.is_retryable());
        assert_eq!(error.error_code(), "BASE_FETCH_FAILED");
        assert_eq!(error.retry_delay_seconds(), Some(1));
    }

    #[test]
    fn test_below_minimum_floor_error() {
        let error = GasError::below_minimum_floor("1000000000", "2000000000");
        assert!(error.is_validation_error());
        assert!(!error.is_retryable());
        assert_eq!(error.error_code(), "BELOW_MINIMUM_FLOOR");
    }

    #[test]
    fn test_priority_fee_exceeds_max_error() {
        let error = GasError::priority_fee_exceeds_max("30", "20");
        assert!(error.is_validation_error());
        assert!(!error.is_retryable());
        assert_eq!(error.error_code(), "PRIORITY_FEE_EXCEEDS_MAX");
    }

    #[test]
    fn test_invalid_configuration_error() {
        let error =
            GasError::invalid_configuration("min_base_fee_gwei", "0", "must be greater than zero");
        assert!(error.is_validation_error());
        assert!(error.is_configuration_error());
        assert!(!error.is_retryable());
        assert_eq!(error.error_code(), "INVALID_CONFIGURATION");
    }

    #[test]
    fn test_service_unavailable_error() {
        let error = GasError::ServiceUnavailable;
        assert!(!error.is_validation_error());
        assert!(error.is_retryable());
        assert_eq!(error.error_code(), "SERVICE_UNAVAILABLE");
        assert_eq!(error.retry_delay_seconds(), Some(5));
    }

    #[test]
    fn test_stale_data_error() {
        let error = GasError::stale_data(120, 60);
        assert!(!error.is_validation_error());
        assert!(!error.is_retryable());
        assert_eq!(error.error_code(), "STALE_DATA");
    }
}
