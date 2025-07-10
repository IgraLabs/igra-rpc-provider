use serde::Deserialize;

/// HTTP server configuration
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ServerConfig {
    /// Server host address
    pub host: String,
    /// Server port
    pub port: u16,
}

impl ServerConfig {
    /// Create a new ServerConfig with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a ServerConfig with specific host and port
    pub fn with_address(host: String, port: u16) -> Self {
        Self { host, port }
    }

    /// Get the full server address
    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// Validate the server configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.host.is_empty() {
            return Err("Server host cannot be empty".to_string());
        }

        if self.port == 0 {
            return Err("Server port cannot be zero".to_string());
        }

        // Note: u16 port type automatically ensures valid range (0-65535)

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_config_creation() {
        let config = ServerConfig::new();
        assert!(config.host.is_empty());
        assert_eq!(config.port, 0);
    }

    #[test]
    fn test_server_config_with_address() {
        let config = ServerConfig::with_address("localhost".to_string(), 8080);
        assert_eq!(config.host, "localhost");
        assert_eq!(config.port, 8080);
        assert_eq!(config.address(), "localhost:8080");
    }

    #[test]
    fn test_server_config_validation_valid() {
        let config = ServerConfig::with_address("0.0.0.0".to_string(), 8080);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_server_config_validation_empty_host() {
        let config = ServerConfig::with_address("".to_string(), 8080);
        assert!(config.validate().is_err());
        assert!(config
            .validate()
            .expect_err("Expected validation to fail")
            .contains("host cannot be empty"));
    }

    #[test]
    fn test_server_config_validation_zero_port() {
        let config = ServerConfig::with_address("localhost".to_string(), 0);
        assert!(config.validate().is_err());
        assert!(config
            .validate()
            .expect_err("Expected validation to fail")
            .contains("port cannot be zero"));
    }

    // Note: u16 type automatically prevents invalid port numbers > 65535
    // This test is no longer relevant since 65536 won't compile
}
