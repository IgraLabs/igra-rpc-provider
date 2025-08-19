use crate::error::AppError;
use serde::Deserialize;
use std::time::Duration;

const DEFAULT_REQUIRED_PREFIX: &[u8] = &[0x97, 0xb1];
const DEFAULT_TIMEOUT_SECONDS: u64 = 10;
const HASH_SIZE: usize = 32;

/// Mining configuration
#[derive(Debug, Clone, Deserialize)]
pub struct MiningConfig {
    /// Required prefix for mining (hash must start with these bytes)
    #[serde(default = "default_required_prefix")]
    pub required_prefix: Vec<u8>,
    /// Mining timeout in seconds
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
}

impl Default for MiningConfig {
    fn default() -> Self {
        Self {
            required_prefix: DEFAULT_REQUIRED_PREFIX.to_vec(),
            timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
        }
    }
}

impl MiningConfig {
    /// Create a new MiningConfig with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a MiningConfig with specific prefix and timeout
    pub fn with_settings(required_prefix: Vec<u8>, timeout_seconds: u64) -> Self {
        Self {
            required_prefix,
            timeout_seconds,
        }
    }

    /// Create a MiningConfig with hex prefix string
    pub fn with_hex_prefix(hex_prefix: &str, timeout_seconds: u64) -> Result<Self, String> {
        let prefix = Self::parse_hex_prefix(hex_prefix)?;
        Ok(Self::with_settings(prefix, timeout_seconds))
    }

    /// Parse hex string to bytes
    pub fn parse_hex_prefix(hex_prefix: &str) -> Result<Vec<u8>, String> {
        let clean_hex = hex_prefix.strip_prefix("0x").unwrap_or(hex_prefix);

        if clean_hex.len() % 2 != 0 {
            return Err("Hex prefix must have even number of characters".to_string());
        }

        hex::decode(clean_hex).map_err(|e| format!("Invalid hex prefix: {e}"))
    }

    /// Validates the mining configuration parameters
    pub fn validate(&self) -> Result<(), AppError> {
        // Validate required_prefix length (empty prefix disabled, max 32 bytes for Kaspa hashes)
        if self.required_prefix.is_empty() {
            return Err(AppError::ConfigError(
                "Mining required_prefix cannot be empty".to_string(),
            ));
        }

        if self.required_prefix.len() > HASH_SIZE {
            return Err(AppError::ConfigError(format!(
                "Mining required_prefix cannot exceed {} bytes (Kaspa hash size), got {} bytes",
                HASH_SIZE,
                self.required_prefix.len()
            )));
        }

        // Validate timeout is reasonable (1-300 seconds)
        if self.timeout_seconds == 0 || self.timeout_seconds > 300 {
            return Err(AppError::ConfigError(format!(
                "Mining timeout_seconds must be between 1-300 seconds, got {} seconds",
                self.timeout_seconds
            )));
        }

        Ok(())
    }

    /// Get timeout as Duration
    pub fn timeout_duration(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds)
    }

    /// Get the required prefix as hex string
    pub fn prefix_hex(&self) -> String {
        hex::encode(&self.required_prefix)
    }

    /// Get the required prefix bytes
    pub fn prefix_bytes(&self) -> &[u8] {
        &self.required_prefix
    }

    /// Get the prefix length
    pub fn prefix_length(&self) -> usize {
        self.required_prefix.len()
    }

    /// Check if a hash matches the required prefix
    pub fn hash_matches_prefix(&self, hash: &[u8]) -> bool {
        if hash.len() < self.required_prefix.len() {
            return false;
        }

        hash.starts_with(&self.required_prefix)
    }

    /// Calculate mining difficulty based on prefix length
    pub fn difficulty_estimate(&self) -> u64 {
        // Each byte of prefix increases difficulty by factor of 256
        // For prefix [0x97, 0xb1], difficulty is approximately 256^2 = 65536
        let len = u32::try_from(self.required_prefix.len()).unwrap_or(0);
        256_u64.pow(len)
    }

    /// Estimate mining time based on hash rate (hashes per second)
    pub fn estimated_mining_time(&self, hash_rate: u64) -> Duration {
        if hash_rate == 0 {
            return Duration::from_secs(u64::MAX);
        }

        let difficulty = self.difficulty_estimate();
        let expected_attempts = difficulty.saturating_div(2); // On average, need half the difficulty attempts
        let seconds = if hash_rate == 0 {
            u64::MAX // Return maximum time for zero hash rate
        } else {
            expected_attempts.checked_div(hash_rate).unwrap_or(u64::MAX)
        };

        Duration::from_secs(seconds.max(1))
    }
}

fn default_required_prefix() -> Vec<u8> {
    DEFAULT_REQUIRED_PREFIX.to_vec()
}

fn default_timeout_seconds() -> u64 {
    DEFAULT_TIMEOUT_SECONDS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mining_config_creation() {
        let config = MiningConfig::new();
        assert_eq!(config.required_prefix, DEFAULT_REQUIRED_PREFIX);
        assert_eq!(config.timeout_seconds, DEFAULT_TIMEOUT_SECONDS);
    }

    #[test]
    fn test_mining_config_with_settings() {
        let prefix = vec![0x12, 0x34];
        let config = MiningConfig::with_settings(prefix.clone(), 30);
        assert_eq!(config.required_prefix, prefix);
        assert_eq!(config.timeout_seconds, 30);
    }

    #[test]
    fn test_mining_config_with_hex_prefix() {
        let config = MiningConfig::with_hex_prefix("0x1234", 30).expect("Should parse hex");
        assert_eq!(config.required_prefix, vec![0x12, 0x34]);
        assert_eq!(config.timeout_seconds, 30);
    }

    #[test]
    fn test_parse_hex_prefix_valid() {
        assert_eq!(
            MiningConfig::parse_hex_prefix("0x1234").expect("Expected valid hex prefix"),
            vec![0x12, 0x34]
        );
        assert_eq!(
            MiningConfig::parse_hex_prefix("1234").expect("Expected valid hex prefix"),
            vec![0x12, 0x34]
        );
        assert_eq!(
            MiningConfig::parse_hex_prefix("0xab").expect("Expected valid hex prefix"),
            vec![0xab]
        );
    }

    #[test]
    fn test_parse_hex_prefix_invalid() {
        assert!(MiningConfig::parse_hex_prefix("0x123").is_err()); // Odd length
        assert!(MiningConfig::parse_hex_prefix("0xgg").is_err()); // Invalid hex
        assert!(MiningConfig::parse_hex_prefix("xyz").is_err()); // Invalid hex
    }

    #[test]
    fn test_mining_config_validation_valid() {
        let config = MiningConfig::with_settings(vec![0x97, 0xb1], 10);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_mining_config_validation_empty_prefix() {
        let config = MiningConfig::with_settings(vec![], 10);
        assert!(config.validate().is_err());
        let err = config
            .validate()
            .expect_err("Expected validation to fail")
            .to_string();
        assert!(err.contains("cannot be empty"));
    }

    #[test]
    fn test_mining_config_validation_prefix_too_long() {
        let long_prefix = vec![0u8; HASH_SIZE + 1];
        let config = MiningConfig::with_settings(long_prefix, 10);
        assert!(config.validate().is_err());
        let err = config
            .validate()
            .expect_err("Expected validation to fail")
            .to_string();
        assert!(err.contains("cannot exceed"));
    }

    #[test]
    fn test_mining_config_validation_invalid_timeout() {
        let config = MiningConfig::with_settings(vec![0x97], 0);
        assert!(config.validate().is_err());

        let config = MiningConfig::with_settings(vec![0x97], 301);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_timeout_duration() {
        let config = MiningConfig::with_settings(vec![0x97], 30);
        assert_eq!(config.timeout_duration(), Duration::from_secs(30));
    }

    #[test]
    fn test_prefix_methods() {
        let config = MiningConfig::with_settings(vec![0x97, 0xb1], 10);
        assert_eq!(config.prefix_hex(), "97b1");
        assert_eq!(config.prefix_bytes(), &[0x97, 0xb1]);
        assert_eq!(config.prefix_length(), 2);
    }

    #[test]
    fn test_hash_matches_prefix() {
        let config = MiningConfig::with_settings(vec![0x97, 0xb1], 10);

        // Hash that matches prefix
        let matching_hash = [0x97, 0xb1, 0x12, 0x34, 0x56, 0x78];
        assert!(config.hash_matches_prefix(&matching_hash));

        // Hash that doesn't match prefix
        let non_matching_hash = [0x96, 0xb1, 0x12, 0x34, 0x56, 0x78];
        assert!(!config.hash_matches_prefix(&non_matching_hash));

        // Hash too short
        let short_hash = [0x97];
        assert!(!config.hash_matches_prefix(&short_hash));
    }

    #[test]
    fn test_difficulty_estimate() {
        let config1 = MiningConfig::with_settings(vec![0x97], 10);
        assert_eq!(config1.difficulty_estimate(), 256);

        let config2 = MiningConfig::with_settings(vec![0x97, 0xb1], 10);
        assert_eq!(config2.difficulty_estimate(), 256 * 256);
    }

    #[test]
    fn test_estimated_mining_time() {
        let config = MiningConfig::with_settings(vec![0x97], 10); // difficulty = 256
        let hash_rate = 256; // 256 hashes per second

        // Expected attempts = 256/2 = 128, time = 128/256 = 0.5s, but min is 1s
        let time = config.estimated_mining_time(hash_rate);
        assert_eq!(time, Duration::from_secs(1));

        // Test with zero hash rate
        let time_zero = config.estimated_mining_time(0);
        assert_eq!(time_zero, Duration::from_secs(u64::MAX));
    }
}
