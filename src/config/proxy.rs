use serde::Deserialize;
use std::time::Duration;

/// EL proxy configuration
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ProxyConfig {
    /// EL URL to proxy requests to
    #[serde(alias = "url")]
    pub el_url: String,
    /// EL WebSocket URL for subscription proxying
    #[serde(default)]
    pub el_ws_url: String,
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
            el_ws_url: String::new(),
            timeout_seconds: default_timeout_seconds(),
            max_retries: default_max_retries(),
            retry_delay_ms: default_retry_delay_ms(),
        }
    }

    /// Create a ProxyConfig with specific EL URL
    pub fn with_el_url(el_url: String) -> Self {
        Self {
            el_url,
            el_ws_url: String::new(),
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
            el_ws_url: String::new(),
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

        // Validate WS URL scheme (only ws:// supported — TLS not enabled for local reth connections)
        if !self.el_ws_url.is_empty() && !self.el_ws_url.starts_with("ws://") {
            return Err("EL WS URL must start with ws:// (wss:// is not supported)".to_string());
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

    /// Get the EL WebSocket URL, deriving from el_url if not explicitly set
    pub fn el_ws_url(&self) -> String {
        if self.el_ws_url.is_empty() {
            derive_ws_url(&self.el_url)
        } else {
            self.el_ws_url.clone()
        }
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

/// Derive a WebSocket URL from an HTTP URL.
/// Always produces `ws://` since wss:// is not supported for local reth connections.
/// Replaces port 8545 with 8546 only in the authority section (host:port).
fn derive_ws_url(http_url: &str) -> String {
    // Strip the scheme — always use ws:// regardless of http/https
    let without_scheme = if let Some(rest) = http_url.strip_prefix("https://") {
        rest
    } else if let Some(rest) = http_url.strip_prefix("http://") {
        rest
    } else {
        http_url
    };

    // Split authority from path at the first '/'
    let (authority, path) = match without_scheme.find('/') {
        Some(idx) => (&without_scheme[..idx], &without_scheme[idx..]),
        None => (without_scheme, ""),
    };

    // Replace port 8545→8546 only if it's the trailing port in the authority
    let authority = if let Some(host) = authority.strip_suffix(":8545") {
        format!("{host}:8546")
    } else {
        authority.to_string()
    };

    format!("ws://{authority}{path}")
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

    #[test]
    fn test_derive_ws_url_http() {
        assert_eq!(
            derive_ws_url("http://localhost:8545"),
            "ws://localhost:8546"
        );
    }

    #[test]
    fn test_derive_ws_url_https() {
        // https:// also derives to ws:// since wss:// is not supported
        assert_eq!(
            derive_ws_url("https://example.com:8545"),
            "ws://example.com:8546"
        );
    }

    #[test]
    fn test_derive_ws_url_non_standard_port() {
        // Port 9545 should not be replaced
        assert_eq!(
            derive_ws_url("http://localhost:9545"),
            "ws://localhost:9545"
        );
    }

    #[test]
    fn test_derive_ws_url_with_path() {
        assert_eq!(
            derive_ws_url("http://localhost:8545/rpc"),
            "ws://localhost:8546/rpc"
        );
    }

    #[test]
    fn test_derive_ws_url_port_in_path_not_replaced() {
        // Port 8545 appearing in a path segment must NOT be replaced
        assert_eq!(
            derive_ws_url("http://localhost:9000/proxy:8545"),
            "ws://localhost:9000/proxy:8545"
        );
    }

    #[test]
    fn test_el_ws_url_derived_when_empty() {
        let config = ProxyConfig::with_el_url("http://localhost:8545".to_string());
        assert_eq!(config.el_ws_url(), "ws://localhost:8546");
    }

    #[test]
    fn test_el_ws_url_explicit() {
        let mut config = ProxyConfig::with_el_url("http://localhost:8545".to_string());
        config.el_ws_url = "ws://custom-host:9999".to_string();
        assert_eq!(config.el_ws_url(), "ws://custom-host:9999");
    }

    #[test]
    fn test_with_el_url_does_not_eagerly_derive() {
        let config = ProxyConfig::with_el_url("http://localhost:8545".to_string());
        // Field should be empty — derivation happens lazily via el_ws_url() accessor
        assert!(config.el_ws_url.is_empty());
    }

    #[test]
    fn test_el_ws_url_validation_invalid_scheme() {
        let mut config = create_valid_config();
        config.el_ws_url = "http://localhost:8546".to_string();
        assert!(config.validate().is_err());
        assert!(config
            .validate()
            .expect_err("Expected validation to fail")
            .contains("EL WS URL must start with ws://"));
    }

    #[test]
    fn test_el_ws_url_validation_valid_ws() {
        let mut config = create_valid_config();
        config.el_ws_url = "ws://localhost:8546".to_string();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_el_ws_url_validation_wss_not_supported() {
        let mut config = create_valid_config();
        config.el_ws_url = "wss://example.com:8546".to_string();
        assert!(config.validate().is_err());
        assert!(config
            .validate()
            .expect_err("Expected validation to fail")
            .contains("wss:// is not supported"));
    }
}
