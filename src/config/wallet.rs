use serde::Deserialize;

/// Wallet connection configuration
#[derive(Debug, Clone, Deserialize)]
pub struct WalletConfig {
    /// Wallet daemon URI
    pub wallet_daemon_uri: String,
    /// Default receiving address
    pub to_address: String,
}

impl WalletConfig {
    /// Create a new WalletConfig
    pub fn new(wallet_daemon_uri: String, to_address: String) -> Self {
        Self {
            wallet_daemon_uri,
            to_address,
        }
    }

    /// Validate the wallet configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.wallet_daemon_uri.is_empty() {
            return Err("Wallet daemon URI cannot be empty".to_string());
        }

        if self.to_address.is_empty() {
            return Err("Wallet to_address cannot be empty".to_string());
        }

        // Validate wallet daemon URI format
        if !self.wallet_daemon_uri.starts_with("http://")
            && !self.wallet_daemon_uri.starts_with("https://")
        {
            return Err("Wallet daemon URI must start with http:// or https://".to_string());
        }

        // Basic validation for Kaspa address format (should start with kaspa network prefix)
        if !self.to_address.starts_with("kaspa:")
            && !self.to_address.starts_with("kaspatest:")
            && !self.to_address.starts_with("kaspadev:")
        {
            return Err(
                "Wallet to_address must be a valid Kaspa address (starts with 'kaspa:', 'kaspatest:', or 'kaspadev:')"
                    .to_string(),
            );
        }

        // Validate address length (Kaspa addresses are typically around 60+ characters)
        if self.to_address.len() < 40 {
            return Err(
                "Wallet to_address appears to be too short for a valid Kaspa address".to_string(),
            );
        }

        Ok(())
    }

    /// Check if the wallet is configured for a specific network (mainnet, testnet, etc.)
    pub fn network_type(&self) -> WalletNetwork {
        if self.to_address.starts_with("kaspatest:")
            || self.to_address.contains("testnet")
            || self.to_address.contains("tn")
        {
            WalletNetwork::Testnet
        } else if self.to_address.starts_with("kaspadev:")
            || self.to_address.contains("devnet")
            || self.to_address.contains("dn")
        {
            WalletNetwork::Devnet
        } else {
            WalletNetwork::Mainnet
        }
    }

    /// Get the base URI for the wallet daemon
    pub fn base_uri(&self) -> &str {
        &self.wallet_daemon_uri
    }

    /// Get the receiving address
    pub fn receiving_address(&self) -> &str {
        &self.to_address
    }
}

/// Kaspa network types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalletNetwork {
    Mainnet,
    Testnet,
    Devnet,
}

impl std::fmt::Display for WalletNetwork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WalletNetwork::Mainnet => write!(f, "mainnet"),
            WalletNetwork::Testnet => write!(f, "testnet"),
            WalletNetwork::Devnet => write!(f, "devnet"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_valid_config() -> WalletConfig {
        WalletConfig::new(
            "http://localhost:8082".to_string(),
            "kaspa:qz0s6w8nxqjhvp8gl5xkqq6r5djx5x6x7x8x9x0x1x2x3x4x5x6x7x8x9x0".to_string(),
        )
    }

    #[test]
    fn test_wallet_config_creation() {
        let config = create_valid_config();
        assert_eq!(config.wallet_daemon_uri, "http://localhost:8082");
        assert!(config.to_address.starts_with("kaspa:"));
    }

    #[test]
    fn test_wallet_config_validation_valid() {
        let config = create_valid_config();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_wallet_config_validation_empty_uri() {
        let mut config = create_valid_config();
        config.wallet_daemon_uri = "".to_string();
        assert!(config.validate().is_err());
        assert!(config
            .validate()
            .expect_err("Expected validation to fail")
            .contains("URI cannot be empty"));
    }

    #[test]
    fn test_wallet_config_validation_empty_address() {
        let mut config = create_valid_config();
        config.to_address = "".to_string();
        assert!(config.validate().is_err());
        assert!(config
            .validate()
            .expect_err("Expected validation to fail")
            .contains("to_address cannot be empty"));
    }

    #[test]
    fn test_wallet_config_validation_invalid_uri_format() {
        let mut config = create_valid_config();
        config.wallet_daemon_uri = "ftp://localhost:8082".to_string();
        assert!(config.validate().is_err());
        assert!(config
            .validate()
            .expect_err("Expected validation to fail")
            .contains("must start with http"));
    }

    #[test]
    fn test_wallet_config_validation_invalid_address_format() {
        let mut config = create_valid_config();
        config.to_address = "bitcoin:1234567890".to_string();
        assert!(config.validate().is_err());
        assert!(config
            .validate()
            .expect_err("Expected validation to fail")
            .contains("must be a valid Kaspa address"));
    }

    #[test]
    fn test_wallet_config_validation_short_address() {
        let mut config = create_valid_config();
        config.to_address = "kaspa:short".to_string();
        assert!(config.validate().is_err());
        assert!(config
            .validate()
            .expect_err("Expected validation to fail")
            .contains("too short"));
    }

    #[test]
    fn test_wallet_config_validation_testnet_address() {
        let config = WalletConfig::new(
            "http://localhost:8082".to_string(),
            "kaspatest:qpv8hxvmtvu0tjruup8y5ggqnx9qt5cre32vxrk8073v28w94g99xt57cy60h".to_string(),
        );
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_wallet_config_validation_devnet_address() {
        let config = WalletConfig::new(
            "http://localhost:8082".to_string(),
            "kaspadev:qpv8hxvmtvu0tjruup8y5ggqnx9qt5cre32vxrk8073v28w94g99xt57cy60h".to_string(),
        );
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_network_type_detection() {
        let mainnet_config = WalletConfig::new(
            "http://localhost:8082".to_string(),
            "kaspa:qz0s6w8nxqjhvp8gl5xkqq6r5djx5x6x7x8x9x0x1x2x3x4x5x6x7x8x9x0".to_string(),
        );
        assert_eq!(mainnet_config.network_type(), WalletNetwork::Mainnet);

        let testnet_config = WalletConfig::new(
            "http://localhost:8082".to_string(),
            "kaspatest:qpv8hxvmtvu0tjruup8y5ggqnx9qt5cre32vxrk8073v28w94g99xt57cy60h".to_string(),
        );
        assert_eq!(testnet_config.network_type(), WalletNetwork::Testnet);

        let devnet_config = WalletConfig::new(
            "http://localhost:8082".to_string(),
            "kaspadev:qpv8hxvmtvu0tjruup8y5ggqnx9qt5cre32vxrk8073v28w94g99xt57cy60h".to_string(),
        );
        assert_eq!(devnet_config.network_type(), WalletNetwork::Devnet);

        let testnet_config = WalletConfig::new(
            "http://localhost:8082".to_string(),
            "kaspa:qz0s6w8nxqjhvp8gl5xkqq6r5djx5x6x7x8x9x0x1x2x3x4x5x6x7testnet".to_string(),
        );
        assert_eq!(testnet_config.network_type(), WalletNetwork::Testnet);
    }

    #[test]
    fn test_accessor_methods() {
        let config = create_valid_config();
        assert_eq!(config.base_uri(), "http://localhost:8082");
        assert!(config.receiving_address().starts_with("kaspa:"));
    }
}
