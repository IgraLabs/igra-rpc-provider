use serde::Deserialize;

const DEFAULT_ENABLE_WHITELIST: bool = true;
const DEFAULT_READ_ONLY: bool = false;

/// Security configuration
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SecurityConfig {
    /// Enable method whitelist
    #[serde(default = "default_enable_whitelist")]
    pub enable_whitelist: bool,

    /// Enable read-only mode (blocks all write operations)
    #[serde(default = "default_read_only")]
    pub read_only: bool,
}

impl SecurityConfig {
    /// Create a new SecurityConfig with default values
    pub fn new() -> Self {
        Self {
            enable_whitelist: default_enable_whitelist(),
            read_only: default_read_only(),
        }
    }

    /// Create a SecurityConfig with whitelist enabled/disabled
    pub fn with_whitelist(enable_whitelist: bool) -> Self {
        Self {
            enable_whitelist,
            read_only: default_read_only(),
        }
    }

    /// Validate the security configuration
    pub fn validate(&self) -> Result<(), String> {
        // No validation needed for simple boolean flag
        Ok(())
    }

    /// Check if method whitelist is enabled
    pub fn whitelist_enabled(&self) -> bool {
        self.enable_whitelist
    }

    /// Check if read-only mode is enabled
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }
}

// Default functions
fn default_enable_whitelist() -> bool {
    DEFAULT_ENABLE_WHITELIST
}

fn default_read_only() -> bool {
    DEFAULT_READ_ONLY
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_security_config_creation() {
        let config = SecurityConfig::new();
        assert_eq!(config.enable_whitelist, DEFAULT_ENABLE_WHITELIST);
        assert_eq!(config.read_only, DEFAULT_READ_ONLY);
    }

    #[test]
    fn test_security_config_with_whitelist() {
        let config = SecurityConfig::with_whitelist(false);
        assert!(!config.enable_whitelist);
    }

    #[test]
    fn test_security_config_validation_valid() {
        let config = SecurityConfig::new();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_accessor_methods() {
        let config = SecurityConfig::new();
        assert!(config.whitelist_enabled());
        assert!(!config.is_read_only());
    }

    #[test]
    fn test_read_only_config() {
        let config = SecurityConfig {
            enable_whitelist: true,
            read_only: true,
        };
        assert!(config.is_read_only());
    }

    #[test]
    fn test_read_only_default() {
        let config = SecurityConfig::new();
        assert!(!config.is_read_only());
        assert_eq!(config.read_only, DEFAULT_READ_ONLY);
    }

    #[test]
    fn test_read_only_with_whitelist() {
        // Test that read_only is independent of whitelist setting
        let config1 = SecurityConfig::with_whitelist(true);
        assert!(!config1.is_read_only());

        let config2 = SecurityConfig::with_whitelist(false);
        assert!(!config2.is_read_only());
    }
}
