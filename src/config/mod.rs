/// Domain-specific configuration modules
///
/// This module organizes configuration types by domain, following the single
/// responsibility principle. Each configuration module handles a specific
/// aspect of the application configuration.
pub mod app;
pub mod gas;
pub mod lane;
pub mod mining;
pub mod proxy;
pub mod retry;
pub mod security;
pub mod server;
pub mod wallet;

// Re-export main app configuration
pub use app::{AppConfig, ElConfig};

// Re-export domain-specific configuration types for easier access
pub use gas::GasConfig;
pub use lane::LaneConfig;
pub use mining::MiningConfig;
pub use proxy::ProxyConfig;
pub use retry::RetryConfig;
pub use security::SecurityConfig;
pub use server::ServerConfig;
pub use wallet::{WalletConfig, WalletNetwork};

/// Validation trait for configuration types
pub trait ConfigValidation {
    /// Validate the configuration and return any errors
    fn validate(&self) -> Result<(), String>;
}

// Implement ConfigValidation for all config types
impl ConfigValidation for GasConfig {
    fn validate(&self) -> Result<(), String> {
        self.validate()
    }
}

impl ConfigValidation for MiningConfig {
    fn validate(&self) -> Result<(), String> {
        self.validate().map_err(|e| e.to_string())
    }
}

impl ConfigValidation for LaneConfig {
    fn validate(&self) -> Result<(), String> {
        self.validate()
    }
}

impl ConfigValidation for ProxyConfig {
    fn validate(&self) -> Result<(), String> {
        self.validate()
    }
}

impl ConfigValidation for SecurityConfig {
    fn validate(&self) -> Result<(), String> {
        self.validate()
    }
}

impl ConfigValidation for ServerConfig {
    fn validate(&self) -> Result<(), String> {
        self.validate()
    }
}

impl ConfigValidation for WalletConfig {
    fn validate(&self) -> Result<(), String> {
        self.validate()
    }
}

impl ConfigValidation for RetryConfig {
    fn validate(&self) -> Result<(), String> {
        self.validate()
    }
}

/// Validate all configuration types in a collection
pub fn validate_all_configs<T: ConfigValidation>(configs: &[T]) -> Result<(), Vec<String>> {
    let errors: Vec<String> = configs
        .iter()
        .enumerate()
        .filter_map(|(i, config)| config.validate().err().map(|e| format!("Config {i}: {e}")))
        .collect();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_validation_trait() {
        let gas_config = GasConfig::new();
        assert!(gas_config.validate().is_ok());

        let server_config = ServerConfig::with_address("localhost".to_string(), 8080);
        assert!(server_config.validate().is_ok());

        let wallet_config = WalletConfig::new(
            "http://localhost:8082".to_string(),
            "kaspa:qz0s6w8nxqjhvp8gl5xkqq6r5djx5x6x7x8x9x0x1x2x3x4x5x6x7x8x9x0".to_string(),
        );
        assert!(wallet_config.validate().is_ok());

        let proxy_config = ProxyConfig::with_el_url("http://localhost:8545".to_string());
        assert!(proxy_config.validate().is_ok());

        let mining_config = MiningConfig::new();
        assert!(mining_config.validate().is_ok());

        let security_config = SecurityConfig::new();
        assert!(security_config.validate().is_ok());
    }

    #[test]
    fn test_validate_all_configs_success() {
        let configs = vec![
            GasConfig::new(),
            GasConfig::with_min_protocol_fee_per_gas_gwei(50),
        ];
        assert!(validate_all_configs(&configs).is_ok());
    }

    #[test]
    fn test_validate_all_configs_failure() {
        let configs = vec![
            GasConfig::with_min_protocol_fee_per_gas_gwei(50), // Valid
            GasConfig::with_min_protocol_fee_per_gas_gwei(0),  // Invalid - zero
        ];
        let result = validate_all_configs(&configs);
        assert!(result.is_err());
        let errors = result.expect_err("Expected validation to fail");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("Config 1:"));
        assert!(errors[0].contains("cannot be zero"));
    }

    #[test]
    fn test_all_config_types_importable() {
        // Test that all config types can be created and used
        let _gas = GasConfig::new();
        let _mining = MiningConfig::new();
        let _proxy = ProxyConfig::new();
        let _security = SecurityConfig::new();
        let _server = ServerConfig::new();
        let _wallet = WalletConfig::new(
            "http://localhost:8082".to_string(),
            "kaspa:qz0s6w8nxqjhvp8gl5xkqq6r5djx5x6x7x8x9x0x1x2x3x4x5x6x7x8x9x0".to_string(),
        );

        // Test network type enum
        let network = WalletNetwork::Mainnet;
        assert_eq!(network.to_string(), "mainnet");
    }
}
