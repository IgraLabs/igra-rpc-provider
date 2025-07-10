use serde::Deserialize;

const DEFAULT_MIN_BASE_FEE_GWEI: u64 = 100;
const MAX_REASONABLE_BASE_FEE_GWEI: u64 = 10_000; // 10,000 gwei = 0.01 ETH

/// Gas pricing configuration
#[derive(Debug, Clone, Deserialize)]
pub struct GasConfig {
    /// Minimum base fee in gwei
    #[serde(default = "default_min_base_fee_gwei")]
    pub min_base_fee_gwei: u64,
}

impl Default for GasConfig {
    fn default() -> Self {
        Self {
            min_base_fee_gwei: default_min_base_fee_gwei(),
        }
    }
}

impl GasConfig {
    /// Create a new GasConfig with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a GasConfig with specific minimum base fee
    pub fn with_min_base_fee_gwei(min_base_fee_gwei: u64) -> Self {
        Self { min_base_fee_gwei }
    }

    /// Get the minimum base fee in wei (gwei * 10^9)
    pub fn min_base_fee_wei(&self) -> u128 {
        u128::from(self.min_base_fee_gwei).saturating_mul(1_000_000_000)
    }

    /// Validate the gas configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.min_base_fee_gwei == 0 {
            return Err("Minimum base fee cannot be zero".to_string());
        }

        if self.min_base_fee_gwei > MAX_REASONABLE_BASE_FEE_GWEI {
            return Err(format!(
                "Minimum base fee is unreasonably high: {} gwei (max reasonable: {} gwei)",
                self.min_base_fee_gwei, MAX_REASONABLE_BASE_FEE_GWEI
            ));
        }

        Ok(())
    }

    /// Check if a gas price in gwei meets the minimum requirement
    pub fn meets_minimum_gwei(&self, gas_price_gwei: u64) -> bool {
        gas_price_gwei >= self.min_base_fee_gwei
    }

    /// Check if a gas price in wei meets the minimum requirement
    pub fn meets_minimum_wei(&self, gas_price_wei: u128) -> bool {
        gas_price_wei >= self.min_base_fee_wei()
    }
}

fn default_min_base_fee_gwei() -> u64 {
    DEFAULT_MIN_BASE_FEE_GWEI
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gas_config_creation() {
        let config = GasConfig::new();
        assert_eq!(config.min_base_fee_gwei, DEFAULT_MIN_BASE_FEE_GWEI);
    }

    #[test]
    fn test_gas_config_with_min_base_fee() {
        let config = GasConfig::with_min_base_fee_gwei(50);
        assert_eq!(config.min_base_fee_gwei, 50);
    }

    #[test]
    fn test_min_base_fee_wei_conversion() {
        let config = GasConfig::with_min_base_fee_gwei(100);
        assert_eq!(config.min_base_fee_wei(), 100_000_000_000); // 100 gwei in wei
    }

    #[test]
    fn test_gas_config_validation_valid() {
        let config = GasConfig::with_min_base_fee_gwei(50);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_gas_config_validation_zero() {
        let config = GasConfig::with_min_base_fee_gwei(0);
        assert!(config.validate().is_err());
        assert!(config
            .validate()
            .expect_err("Expected validation to fail")
            .contains("cannot be zero"));
    }

    #[test]
    fn test_gas_config_validation_too_high() {
        let config = GasConfig::with_min_base_fee_gwei(MAX_REASONABLE_BASE_FEE_GWEI + 1);
        assert!(config.validate().is_err());
        assert!(config
            .validate()
            .expect_err("Expected validation to fail")
            .contains("unreasonably high"));
    }

    #[test]
    fn test_meets_minimum_gwei() {
        let config = GasConfig::with_min_base_fee_gwei(100);
        assert!(config.meets_minimum_gwei(100));
        assert!(config.meets_minimum_gwei(150));
        assert!(!config.meets_minimum_gwei(50));
    }

    #[test]
    fn test_meets_minimum_wei() {
        let config = GasConfig::with_min_base_fee_gwei(100);
        assert!(config.meets_minimum_wei(100_000_000_000)); // 100 gwei
        assert!(config.meets_minimum_wei(150_000_000_000)); // 150 gwei
        assert!(!config.meets_minimum_wei(50_000_000_000)); // 50 gwei
    }
}
