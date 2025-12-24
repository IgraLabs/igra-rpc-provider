use crate::error::AppError;
use serde::{Deserialize, Deserializer};
use std::time::Duration;

const DEFAULT_TX_ID_PREFIX: &[u8] = &[0x97, 0xb1];
const DEFAULT_TIMEOUT_SECONDS: u64 = 10;
const HASH_SIZE: usize = 32;

/// Mining configuration
#[derive(Debug, Clone, Deserialize)]
pub struct MiningConfig {
    /// Required prefix for transaction ID (hash must start with these bytes)
    /// Accepts both array format [0x97, 0xb1] and hex string "97b1" or "0x97b1"
    #[serde(
        default = "default_tx_id_prefix",
        deserialize_with = "deserialize_tx_id_prefix"
    )]
    pub tx_id_prefix: Vec<u8>,
    /// Mining timeout in seconds
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
}

/// Custom deserializer for tx_id_prefix that handles both array and hex string formats
///
/// # Supported Formats
/// - Hex string without prefix: `"97b1"` → `[0x97, 0xb1]`
/// - Hex string with prefix: `"0x97b1"` → `[0x97, 0xb1]`
/// - Byte array: `[151, 177]` → `[0x97, 0xb1]`
///
/// # Validation Rules
/// - Prefix cannot be empty (minimum 1 byte)
/// - Prefix cannot exceed 32 bytes (Kaspa hash size)
/// - Hex strings must have even length (each byte = 2 hex digits)
fn deserialize_tx_id_prefix<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::{self, SeqAccess, Visitor};
    use std::fmt;

    struct TxIdPrefixVisitor;

    impl<'de> Visitor<'de> for TxIdPrefixVisitor {
        type Value = Vec<u8>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str(
                "a hex string like \"97b1\" or \"0x97b1\", or an array of bytes like [0x97, 0xb1]",
            )
        }

        // Handle string format: "97b1" or "0x97b1"
        fn visit_str<E>(self, value: &str) -> Result<Vec<u8>, E>
        where
            E: de::Error,
        {
            let clean_hex = value.strip_prefix("0x").unwrap_or(value);

            if clean_hex.is_empty() {
                return Err(de::Error::custom("tx_id_prefix cannot be empty"));
            }

            if !clean_hex.len().is_multiple_of(2) {
                return Err(de::Error::custom(
                    "hex string must have even number of characters (each byte is 2 hex digits)",
                ));
            }

            let bytes = hex::decode(clean_hex)
                .map_err(|e| de::Error::custom(format!("invalid hex string: {e}")))?;

            if bytes.len() > HASH_SIZE {
                return Err(de::Error::custom(format!(
                    "tx_id_prefix cannot exceed {} bytes, got {} bytes",
                    HASH_SIZE,
                    bytes.len()
                )));
            }

            Ok(bytes)
        }

        // Handle array format: [0x97, 0xb1]
        fn visit_seq<A>(self, mut seq: A) -> Result<Vec<u8>, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut bytes = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(HASH_SIZE));
            while let Some(byte) = seq.next_element()? {
                bytes.push(byte);
                if bytes.len() > HASH_SIZE {
                    return Err(de::Error::custom(format!(
                        "tx_id_prefix cannot exceed {} bytes",
                        HASH_SIZE
                    )));
                }
            }

            if bytes.is_empty() {
                return Err(de::Error::custom("tx_id_prefix cannot be empty"));
            }

            Ok(bytes)
        }
    }

    deserializer.deserialize_any(TxIdPrefixVisitor)
}

impl Default for MiningConfig {
    fn default() -> Self {
        Self {
            tx_id_prefix: DEFAULT_TX_ID_PREFIX.to_vec(),
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
    pub fn with_settings(tx_id_prefix: Vec<u8>, timeout_seconds: u64) -> Self {
        Self {
            tx_id_prefix,
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

        if !clean_hex.len().is_multiple_of(2) {
            return Err("Hex prefix must have even number of characters".to_string());
        }

        hex::decode(clean_hex).map_err(|e| format!("Invalid hex prefix: {e}"))
    }

    /// Validates the mining configuration parameters
    pub fn validate(&self) -> Result<(), AppError> {
        // Validate tx_id_prefix length (empty prefix disabled, max 32 bytes for Kaspa hashes)
        if self.tx_id_prefix.is_empty() {
            return Err(AppError::ConfigError(
                "Mining tx_id_prefix cannot be empty".to_string(),
            ));
        }

        if self.tx_id_prefix.len() > HASH_SIZE {
            return Err(AppError::ConfigError(format!(
                "Mining tx_id_prefix cannot exceed {} bytes (Kaspa hash size), got {} bytes",
                HASH_SIZE,
                self.tx_id_prefix.len()
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
        hex::encode(&self.tx_id_prefix)
    }

    /// Get the required prefix bytes
    pub fn prefix_bytes(&self) -> &[u8] {
        &self.tx_id_prefix
    }

    /// Get the prefix length
    pub fn prefix_length(&self) -> usize {
        self.tx_id_prefix.len()
    }

    /// Check if a hash matches the required prefix
    pub fn hash_matches_prefix(&self, hash: &[u8]) -> bool {
        if hash.len() < self.tx_id_prefix.len() {
            return false;
        }

        hash.starts_with(&self.tx_id_prefix)
    }

    /// Calculate mining difficulty based on prefix length
    pub fn difficulty_estimate(&self) -> u64 {
        // Each byte of prefix increases difficulty by factor of 256
        // For prefix [0x97, 0xb1], difficulty is approximately 256^2 = 65536
        let len = u32::try_from(self.tx_id_prefix.len()).unwrap_or(0);
        256_u64.pow(len)
    }

    /// Estimate mining time based on hash rate (hashes per second)
    pub fn estimated_mining_time(&self, hash_rate: u64) -> Duration {
        if hash_rate == 0 {
            return Duration::from_secs(u64::MAX);
        }

        let difficulty = self.difficulty_estimate();
        let expected_attempts = difficulty.saturating_div(2); // On average, need half the difficulty attempts
        let seconds = expected_attempts.checked_div(hash_rate).unwrap_or(u64::MAX);

        Duration::from_secs(seconds.max(1))
    }
}

fn default_tx_id_prefix() -> Vec<u8> {
    DEFAULT_TX_ID_PREFIX.to_vec()
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
        assert_eq!(config.tx_id_prefix, DEFAULT_TX_ID_PREFIX);
        assert_eq!(config.timeout_seconds, DEFAULT_TIMEOUT_SECONDS);
    }

    #[test]
    fn test_mining_config_with_settings() {
        let prefix = vec![0x12, 0x34];
        let config = MiningConfig::with_settings(prefix.clone(), 30);
        assert_eq!(config.tx_id_prefix, prefix);
        assert_eq!(config.timeout_seconds, 30);
    }

    #[test]
    fn test_mining_config_with_hex_prefix() {
        let config = MiningConfig::with_hex_prefix("0x1234", 30).expect("Should parse hex");
        assert_eq!(config.tx_id_prefix, vec![0x12, 0x34]);
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

    // ========== Deserialization Tests ==========

    #[test]
    fn test_deserialize_tx_id_prefix_from_hex_string() {
        // Test hex string without 0x prefix
        let json = r#"{"tx_id_prefix": "97b1", "timeout_seconds": 10}"#;
        let config: MiningConfig =
            serde_json::from_str(json).expect("Should deserialize hex string");
        assert_eq!(config.tx_id_prefix, vec![0x97, 0xb1]);

        // Test hex string with 0x prefix
        let json = r#"{"tx_id_prefix": "0x97b1", "timeout_seconds": 10}"#;
        let config: MiningConfig =
            serde_json::from_str(json).expect("Should deserialize 0x hex string");
        assert_eq!(config.tx_id_prefix, vec![0x97, 0xb1]);
    }

    #[test]
    fn test_deserialize_tx_id_prefix_from_array() {
        let json = r#"{"tx_id_prefix": [151, 177], "timeout_seconds": 10}"#;
        let config: MiningConfig = serde_json::from_str(json).expect("Should deserialize array");
        assert_eq!(config.tx_id_prefix, vec![0x97, 0xb1]);
    }

    #[test]
    fn test_deserialize_tx_id_prefix_default() {
        let json = r#"{"timeout_seconds": 10}"#;
        let config: MiningConfig = serde_json::from_str(json).expect("Should use default prefix");
        assert_eq!(config.tx_id_prefix, DEFAULT_TX_ID_PREFIX);
    }

    // ========== Deserialization Edge Case Tests ==========

    #[test]
    fn test_deserialize_tx_id_prefix_empty_string_fails() {
        let json = r#"{"tx_id_prefix": "", "timeout_seconds": 10}"#;
        let result: Result<MiningConfig, _> = serde_json::from_str(json);
        assert!(result.is_err());
        let err = result
            .expect_err("Expected error for empty string")
            .to_string();
        assert!(err.contains("cannot be empty"), "Error was: {err}");
    }

    #[test]
    fn test_deserialize_tx_id_prefix_only_0x_prefix_fails() {
        let json = r#"{"tx_id_prefix": "0x", "timeout_seconds": 10}"#;
        let result: Result<MiningConfig, _> = serde_json::from_str(json);
        assert!(result.is_err());
        let err = result.expect_err("Expected error for 0x only").to_string();
        assert!(err.contains("cannot be empty"), "Error was: {err}");
    }

    #[test]
    fn test_deserialize_tx_id_prefix_odd_length_hex_fails() {
        let json = r#"{"tx_id_prefix": "97b", "timeout_seconds": 10}"#;
        let result: Result<MiningConfig, _> = serde_json::from_str(json);
        assert!(result.is_err());
        let err = result
            .expect_err("Expected error for odd length")
            .to_string();
        assert!(
            err.contains("even number of characters"),
            "Error was: {err}"
        );
    }

    #[test]
    fn test_deserialize_tx_id_prefix_invalid_hex_fails() {
        let json = r#"{"tx_id_prefix": "gg", "timeout_seconds": 10}"#;
        let result: Result<MiningConfig, _> = serde_json::from_str(json);
        assert!(result.is_err());
        let err = result
            .expect_err("Expected error for invalid hex")
            .to_string();
        assert!(err.contains("invalid hex"), "Error was: {err}");
    }

    #[test]
    fn test_deserialize_tx_id_prefix_empty_array_fails() {
        let json = r#"{"tx_id_prefix": [], "timeout_seconds": 10}"#;
        let result: Result<MiningConfig, _> = serde_json::from_str(json);
        assert!(result.is_err());
        let err = result
            .expect_err("Expected error for empty array")
            .to_string();
        assert!(err.contains("cannot be empty"), "Error was: {err}");
    }

    #[test]
    fn test_deserialize_tx_id_prefix_too_long_hex_fails() {
        // 33 bytes = 66 hex characters (exceeds HASH_SIZE of 32)
        let long_hex = "00".repeat(33);
        let json = format!(r#"{{"tx_id_prefix": "{long_hex}", "timeout_seconds": 10}}"#);
        let result: Result<MiningConfig, _> = serde_json::from_str(&json);
        assert!(result.is_err());
        let err = result
            .expect_err("Expected error for too long hex")
            .to_string();
        assert!(err.contains("cannot exceed"), "Error was: {err}");
    }

    #[test]
    fn test_deserialize_tx_id_prefix_too_long_array_fails() {
        // 33 bytes (exceeds HASH_SIZE of 32)
        let long_array: Vec<u8> = vec![0u8; 33];
        let json = format!(
            r#"{{"tx_id_prefix": {:?}, "timeout_seconds": 10}}"#,
            long_array
        );
        let result: Result<MiningConfig, _> = serde_json::from_str(&json);
        assert!(result.is_err());
        let err = result
            .expect_err("Expected error for too long array")
            .to_string();
        assert!(err.contains("cannot exceed"), "Error was: {err}");
    }

    #[test]
    fn test_deserialize_tx_id_prefix_max_valid_length() {
        // 32 bytes = 64 hex characters (exactly HASH_SIZE)
        let max_hex = "00".repeat(32);
        let json = format!(r#"{{"tx_id_prefix": "{max_hex}", "timeout_seconds": 10}}"#);
        let config: MiningConfig =
            serde_json::from_str(&json).expect("Should accept max length prefix");
        assert_eq!(config.tx_id_prefix.len(), 32);
    }

    #[test]
    fn test_deserialize_tx_id_prefix_single_byte() {
        let json = r#"{"tx_id_prefix": "ff", "timeout_seconds": 10}"#;
        let config: MiningConfig =
            serde_json::from_str(json).expect("Should accept single byte prefix");
        assert_eq!(config.tx_id_prefix, vec![0xff]);
    }
}
