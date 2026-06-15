use serde::Deserialize;

/// Default write-path processing timeout in seconds.
///
/// Matches the historical hard-coded value so behavior is unchanged unless an operator overrides
/// it. Operators should set this just below their reverse-proxy read timeout so the app returns a
/// structured JSON-RPC timeout instead of the proxy truncating the response into an empty body.
pub const DEFAULT_PROCESSING_TIMEOUT_SECONDS: u64 = 120;

fn default_processing_timeout_seconds() -> u64 {
    DEFAULT_PROCESSING_TIMEOUT_SECONDS
}

/// HTTP server configuration
#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    /// Server host address
    pub host: String,
    /// Server port
    pub port: u16,
    /// Maximum seconds the write path waits for a queued transaction before returning a JSON-RPC
    /// timeout error. Tune below the reverse-proxy read timeout to avoid empty (truncated)
    /// responses under load.
    #[serde(default = "default_processing_timeout_seconds")]
    pub processing_timeout_seconds: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 0,
            processing_timeout_seconds: DEFAULT_PROCESSING_TIMEOUT_SECONDS,
        }
    }
}

impl ServerConfig {
    /// Create a new ServerConfig with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a ServerConfig with specific host and port
    pub fn with_address(host: String, port: u16) -> Self {
        Self {
            host,
            port,
            ..Default::default()
        }
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

        if self.processing_timeout_seconds == 0 || self.processing_timeout_seconds > 300 {
            return Err(format!(
                "Server processing_timeout_seconds must be between 1 and 300, got {}",
                self.processing_timeout_seconds
            ));
        }

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

    #[test]
    fn test_server_config_default_timeout_is_valid() {
        let config = ServerConfig::with_address("localhost".to_string(), 8080);
        assert_eq!(
            config.processing_timeout_seconds,
            DEFAULT_PROCESSING_TIMEOUT_SECONDS
        );
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_server_config_validation_rejects_zero_timeout() {
        let config = ServerConfig {
            host: "localhost".to_string(),
            port: 8080,
            processing_timeout_seconds: 0,
        };
        assert!(config.validate().is_err());
        assert!(config
            .validate()
            .expect_err("Expected validation to fail")
            .contains("processing_timeout_seconds"));
    }

    #[test]
    fn test_server_config_validation_rejects_excessive_timeout() {
        let config = ServerConfig {
            host: "localhost".to_string(),
            port: 8080,
            processing_timeout_seconds: 301,
        };
        assert!(config.validate().is_err());
    }
}
