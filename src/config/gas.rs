use serde::Deserialize;

const DEFAULT_MIN_PROTOCOL_FEE_PER_GAS_GWEI: u64 = 100;
const MAX_REASONABLE_PROTOCOL_FEE_PER_GAS_GWEI: u64 = 10_000; // 10,000 gwei = 0.01 ETH

/// Gas pricing configuration
#[derive(Debug, Clone, Deserialize)]
pub struct GasConfig {
    /// Minimum protocol fee per gas in gwei
    #[serde(default = "default_min_protocol_fee_per_gas_gwei")]
    pub min_protocol_fee_per_gas_gwei: u64,
}

impl Default for GasConfig {
    fn default() -> Self {
        Self {
            min_protocol_fee_per_gas_gwei: default_min_protocol_fee_per_gas_gwei(),
        }
    }
}

impl GasConfig {
    /// Create a new GasConfig with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a GasConfig with specific minimum protocol fee per gas
    pub fn with_min_protocol_fee_per_gas_gwei(min_protocol_fee_per_gas_gwei: u64) -> Self {
        Self {
            min_protocol_fee_per_gas_gwei,
        }
    }

    /// Get the minimum protocol fee per gas in wei (gwei * 10^9)
    pub fn min_protocol_fee_per_gas_wei(&self) -> u128 {
        u128::from(self.min_protocol_fee_per_gas_gwei).saturating_mul(1_000_000_000)
    }

    /// Validate the gas configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.min_protocol_fee_per_gas_gwei == 0 {
            return Err("Minimum protocol fee per gas cannot be zero".to_string());
        }

        if self.min_protocol_fee_per_gas_gwei > MAX_REASONABLE_PROTOCOL_FEE_PER_GAS_GWEI {
            return Err(format!(
                "Minimum protocol fee per gas is unreasonably high: {} gwei (max reasonable: {} gwei)",
                self.min_protocol_fee_per_gas_gwei, MAX_REASONABLE_PROTOCOL_FEE_PER_GAS_GWEI
            ));
        }

        Ok(())
    }

    /// Check if a gas price in gwei meets the minimum protocol fee requirement
    pub fn meets_minimum_gwei(&self, gas_price_gwei: u64) -> bool {
        gas_price_gwei >= self.min_protocol_fee_per_gas_gwei
    }

    /// Check if a gas price in wei meets the minimum protocol fee requirement
    pub fn meets_minimum_wei(&self, gas_price_wei: u128) -> bool {
        gas_price_wei >= self.min_protocol_fee_per_gas_wei()
    }
}

fn default_min_protocol_fee_per_gas_gwei() -> u64 {
    DEFAULT_MIN_PROTOCOL_FEE_PER_GAS_GWEI
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gas_config_creation() {
        let config = GasConfig::new();
        assert_eq!(
            config.min_protocol_fee_per_gas_gwei,
            DEFAULT_MIN_PROTOCOL_FEE_PER_GAS_GWEI
        );
    }

    #[test]
    fn test_gas_config_with_min_protocol_fee() {
        let config = GasConfig::with_min_protocol_fee_per_gas_gwei(50);
        assert_eq!(config.min_protocol_fee_per_gas_gwei, 50);
    }

    #[test]
    fn test_min_protocol_fee_wei_conversion() {
        let config = GasConfig::with_min_protocol_fee_per_gas_gwei(100);
        assert_eq!(config.min_protocol_fee_per_gas_wei(), 100_000_000_000); // 100 gwei in wei
    }

    #[test]
    fn test_gas_config_validation_valid() {
        let config = GasConfig::with_min_protocol_fee_per_gas_gwei(50);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_gas_config_validation_zero() {
        let config = GasConfig::with_min_protocol_fee_per_gas_gwei(0);
        assert!(config.validate().is_err());
        assert!(config
            .validate()
            .expect_err("Expected validation to fail")
            .contains("cannot be zero"));
    }

    #[test]
    fn test_gas_config_validation_too_high() {
        let config = GasConfig::with_min_protocol_fee_per_gas_gwei(
            MAX_REASONABLE_PROTOCOL_FEE_PER_GAS_GWEI + 1,
        );
        assert!(config.validate().is_err());
        assert!(config
            .validate()
            .expect_err("Expected validation to fail")
            .contains("unreasonably high"));
    }

    #[test]
    fn test_meets_minimum_gwei() {
        let config = GasConfig::with_min_protocol_fee_per_gas_gwei(100);
        assert!(config.meets_minimum_gwei(100));
        assert!(config.meets_minimum_gwei(150));
        assert!(!config.meets_minimum_gwei(50));
    }

    #[test]
    fn test_meets_minimum_wei() {
        let config = GasConfig::with_min_protocol_fee_per_gas_gwei(100);
        assert!(config.meets_minimum_wei(100_000_000_000)); // 100 gwei
        assert!(config.meets_minimum_wei(150_000_000_000)); // 150 gwei
        assert!(!config.meets_minimum_wei(50_000_000_000)); // 50 gwei
    }
}
