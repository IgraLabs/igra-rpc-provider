use serde::Deserialize;
use std::time::Duration;

/// EL proxy configuration
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ProxyConfig {
    /// EL URL to proxy requests to
    #[serde(alias = "url")]
    pub el_url: String,
    /// Request timeout in seconds
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    /// Maximum retries for failed requests
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Retry delay in milliseconds
    #[serde(default = "default_retry_delay_ms")]
    pub retry_delay_ms: u64,
}

impl ProxyConfig {
    /// Create a new ProxyConfig with default values
    pub fn new() -> Self {
        Self {
            el_url: String::new(),
            timeout_seconds: default_timeout_seconds(),
            max_retries: default_max_retries(),
            retry_delay_ms: default_retry_delay_ms(),
        }
    }

    /// Create a ProxyConfig with specific EL URL
    pub fn with_el_url(el_url: String) -> Self {
        Self {
            el_url,
            timeout_seconds: default_timeout_seconds(),
            max_retries: default_max_retries(),
            retry_delay_ms: default_retry_delay_ms(),
        }
    }

    /// Create a ProxyConfig with all parameters
    pub fn with_settings(
        el_url: String,
        timeout_seconds: u64,
        max_retries: u32,
        retry_delay_ms: u64,
    ) -> Self {
        Self {
            el_url,
            timeout_seconds,
            max_retries,
            retry_delay_ms,
        }
    }

    /// Validate the proxy configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.el_url.is_empty() {
            return Err("EL URL cannot be empty".to_string());
        }

        // Validate EL URL format
        if !self.el_url.starts_with("http://") && !self.el_url.starts_with("https://") {
            return Err("EL URL must start with http:// or https://".to_string());
        }

        // Validate timeout is reasonable (1-300 seconds)
        if self.timeout_seconds == 0 || self.timeout_seconds > 300 {
            return Err(format!(
                "Timeout must be between 1-300 seconds, got {}",
                self.timeout_seconds
            ));
        }

        // Validate max retries is reasonable (0-10)
        if self.max_retries > 10 {
            return Err(format!(
                "Max retries must be <= 10, got {}",
                self.max_retries
            ));
        }

        // Validate retry delay is reasonable (10ms - 10s)
        if self.retry_delay_ms < 10 || self.retry_delay_ms > 10_000 {
            return Err(format!(
                "Retry delay must be between 10-10000ms, got {}ms",
                self.retry_delay_ms
            ));
        }

        Ok(())
    }

    /// Get timeout as Duration
    pub fn timeout_duration(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds)
    }

    /// Get retry delay as Duration
    pub fn retry_delay_duration(&self) -> Duration {
        Duration::from_millis(self.retry_delay_ms)
    }

    /// Get the EL URL
    pub fn el_url(&self) -> &str {
        &self.el_url
    }

    /// Check if retries are enabled
    pub fn retries_enabled(&self) -> bool {
        self.max_retries > 0
    }

    /// Get total maximum time including all retries
    pub fn max_total_time(&self) -> Duration {
        let base_timeout = self.timeout_duration();
        let retry_overhead = self.retry_delay_duration().saturating_mul(self.max_retries);
        let retry_timeouts = base_timeout.saturating_mul(self.max_retries);

        base_timeout
            .saturating_add(retry_overhead)
            .saturating_add(retry_timeouts)
    }
}

// Default values
fn default_timeout_seconds() -> u64 {
    30 // 30 second timeout
}

fn default_max_retries() -> u32 {
    3 // 3 retries by default
}

fn default_retry_delay_ms() -> u64 {
    1000 // 1 second retry delay
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_valid_config() -> ProxyConfig {
        ProxyConfig::with_el_url("http://localhost:8545".to_string())
    }

    #[test]
    fn test_proxy_config_creation() {
        let config = ProxyConfig::new();
        assert!(config.el_url.is_empty());
        assert_eq!(config.timeout_seconds, default_timeout_seconds());
        assert_eq!(config.max_retries, default_max_retries());
        assert_eq!(config.retry_delay_ms, default_retry_delay_ms());
    }

    #[test]
    fn test_proxy_config_with_el_url() {
        let config = ProxyConfig::with_el_url("http://localhost:8545".to_string());
        assert_eq!(config.el_url, "http://localhost:8545");
        assert_eq!(config.timeout_seconds, default_timeout_seconds());
    }

    #[test]
    fn test_proxy_config_with_settings() {
        let config = ProxyConfig::with_settings("http://localhost:8545".to_string(), 60, 5, 2000);
        assert_eq!(config.el_url, "http://localhost:8545");
        assert_eq!(config.timeout_seconds, 60);
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.retry_delay_ms, 2000);
    }

    #[test]
    fn test_proxy_config_validation_valid() {
        let config = create_valid_config();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_proxy_config_validation_empty_url() {
        let mut config = create_valid_config();
        config.el_url = "".to_string();
        assert!(config.validate().is_err());
        assert!(config
            .validate()
            .expect_err("Expected validation to fail")
            .contains("URL cannot be empty"));
    }

    #[test]
    fn test_proxy_config_validation_invalid_url_format() {
        let mut config = create_valid_config();
        config.el_url = "ftp://localhost:8545".to_string();
        assert!(config.validate().is_err());
        assert!(config
            .validate()
            .expect_err("Expected validation to fail")
            .contains("must start with http"));
    }

    #[test]
    fn test_proxy_config_validation_invalid_timeout() {
        let mut config = create_valid_config();
        config.timeout_seconds = 0;
        assert!(config.validate().is_err());
        assert!(config
            .validate()
            .expect_err("Expected validation to fail")
            .contains("Timeout must be between"));

        config.timeout_seconds = 301;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_proxy_config_validation_invalid_retries() {
        let mut config = create_valid_config();
        config.max_retries = 11;
        assert!(config.validate().is_err());
        assert!(config
            .validate()
            .expect_err("Expected validation to fail")
            .contains("Max retries must be"));
    }

    #[test]
    fn test_proxy_config_validation_invalid_retry_delay() {
        let mut config = create_valid_config();
        config.retry_delay_ms = 5;
        assert!(config.validate().is_err());
        assert!(config
            .validate()
            .expect_err("Expected validation to fail")
            .contains("Retry delay must be between"));

        config.retry_delay_ms = 20_000;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_duration_methods() {
        let config = ProxyConfig::with_settings("http://localhost:8545".to_string(), 10, 2, 500);

        assert_eq!(config.timeout_duration(), Duration::from_secs(10));
        assert_eq!(config.retry_delay_duration(), Duration::from_millis(500));
    }

    #[test]
    fn test_accessor_methods() {
        let config = create_valid_config();
        assert_eq!(config.el_url(), "http://localhost:8545");
        assert!(config.retries_enabled());
    }

    #[test]
    fn test_max_total_time() {
        let config = ProxyConfig::with_settings(
            "http://localhost:8545".to_string(),
            10,   // 10s timeout
            2,    // 2 retries
            1000, // 1s retry delay
        );

        // Total: 10s + (2 * 1s) + (2 * 10s) = 32s
        let expected = Duration::from_secs(32);
        assert_eq!(config.max_total_time(), expected);
    }

    #[test]
    fn test_retries_disabled() {
        let config = ProxyConfig::with_settings(
            "http://localhost:8545".to_string(),
            10,
            0, // No retries
            1000,
        );
        assert!(!config.retries_enabled());
    }
}
