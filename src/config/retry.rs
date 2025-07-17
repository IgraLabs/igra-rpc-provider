/// Retry configuration for handling transient errors
///
/// This module defines configuration for automatic retry mechanisms,
/// particularly for handling UTXO exhaustion errors in wallet transactions.
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Error types for retry configuration validation
#[derive(Debug, Error)]
pub enum RetryConfigError {
    #[error("max_attempts must be greater than 0")]
    ZeroMaxAttempts,

    #[error("initial_delay_ms must be greater than 0")]
    ZeroInitialDelay,

    #[error("max_delay_ms ({max}) must be >= initial_delay_ms ({initial})")]
    InvalidDelayRange { initial: u64, max: u64 },
}

/// Default maximum retry attempts
const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// Default initial retry delay in milliseconds
const DEFAULT_INITIAL_DELAY_MS: u64 = 100;

/// Default maximum retry delay in milliseconds
const DEFAULT_MAX_DELAY_MS: u64 = 3000;

/// Configuration for retry logic
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetryConfig {
    /// Maximum number of retry attempts
    pub max_attempts: u32,

    /// Initial delay between retries in milliseconds
    pub initial_delay_ms: u64,

    /// Maximum delay between retries in milliseconds
    pub max_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            initial_delay_ms: DEFAULT_INITIAL_DELAY_MS,
            max_delay_ms: DEFAULT_MAX_DELAY_MS,
        }
    }
}

impl RetryConfig {
    /// Create a new RetryConfig with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a RetryConfig with custom values
    pub fn with_values(max_attempts: u32, initial_delay_ms: u64, max_delay_ms: u64) -> Self {
        Self {
            max_attempts,
            initial_delay_ms,
            max_delay_ms,
        }
    }

    /// Validate the retry configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.max_attempts == 0 {
            return Err(RetryConfigError::ZeroMaxAttempts.to_string());
        }

        if self.initial_delay_ms == 0 {
            return Err(RetryConfigError::ZeroInitialDelay.to_string());
        }

        if self.max_delay_ms < self.initial_delay_ms {
            return Err(RetryConfigError::InvalidDelayRange {
                initial: self.initial_delay_ms,
                max: self.max_delay_ms,
            }
            .to_string());
        }

        Ok(())
    }

    /// Calculate the delay for a given retry attempt using exponential backoff
    ///
    /// The delay doubles with each attempt, capped at max_delay_ms
    pub fn calculate_delay_ms(&self, attempt: u32) -> u64 {
        if attempt == 0 {
            return 0;
        }

        // Use bit shifting for efficient power-of-2 calculation
        // Limit shift to 63 to prevent overflow (2^63 is max safe shift for u64)
        let shift = attempt.saturating_sub(1).min(63);
        let exponential_delay = self.initial_delay_ms.saturating_mul(1u64 << shift);
        exponential_delay.min(self.max_delay_ms)
    }

    /// Add jitter to a delay value to avoid thundering herd problem
    ///
    /// Returns a value between 75% and 125% of the input delay
    pub fn add_jitter(&self, delay_ms: u64) -> u64 {
        use rand::Rng;
        let mut rng = rand::thread_rng();

        // Generate jitter between -25% and +25% using integer arithmetic
        let jitter_percent = rng.gen_range(-25i32..=25);

        // Calculate jitter amount safely without floating point
        // Using checked arithmetic to prevent overflow
        let jitter_amount = delay_ms
            .checked_mul(u64::from(jitter_percent.unsigned_abs()))
            .and_then(|v| v.checked_div(100))
            .unwrap_or(0);

        // Apply jitter with saturating arithmetic
        if jitter_percent >= 0 {
            delay_ms.saturating_add(jitter_amount)
        } else {
            delay_ms.saturating_sub(jitter_amount)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RetryConfig::default();
        assert_eq!(config.max_attempts, 3);
        assert_eq!(config.initial_delay_ms, 100);
        assert_eq!(config.max_delay_ms, 3000);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_custom_config() {
        let config = RetryConfig::with_values(5, 500, 60000);
        assert_eq!(config.max_attempts, 5);
        assert_eq!(config.initial_delay_ms, 500);
        assert_eq!(config.max_delay_ms, 60000);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validation_zero_attempts() {
        let config = RetryConfig::with_values(0, 1000, 30000);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validation_zero_initial_delay() {
        let config = RetryConfig::with_values(3, 0, 30000);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validation_max_delay_less_than_initial() {
        let config = RetryConfig::with_values(3, 5000, 1000);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_calculate_delay() {
        let config = RetryConfig::with_values(5, 1000, 10000);

        assert_eq!(config.calculate_delay_ms(0), 0);
        assert_eq!(config.calculate_delay_ms(1), 1000);
        assert_eq!(config.calculate_delay_ms(2), 2000);
        assert_eq!(config.calculate_delay_ms(3), 4000);
        assert_eq!(config.calculate_delay_ms(4), 8000);
        assert_eq!(config.calculate_delay_ms(5), 10000); // Capped at max_delay
        assert_eq!(config.calculate_delay_ms(6), 10000); // Still capped
    }

    #[test]
    fn test_jitter() {
        let config = RetryConfig::default();
        let base_delay = 1000u64;

        // Test multiple times to ensure jitter is working
        let mut different_values = std::collections::HashSet::new();
        for _ in 0..10 {
            let jittered = config.add_jitter(base_delay);
            assert!(jittered >= 750); // 75% of 1000
            assert!(jittered <= 1250); // 125% of 1000
            different_values.insert(jittered);
        }

        // We should get at least some different values
        assert!(different_values.len() > 1);
    }
}
