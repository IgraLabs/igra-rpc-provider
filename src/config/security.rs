use serde::Deserialize;

const DEFAULT_ENABLE_WHITELIST: bool = true;

/// Security configuration
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SecurityConfig {
    /// Enable method whitelist
    #[serde(default = "default_enable_whitelist")]
    pub enable_whitelist: bool,
}

impl SecurityConfig {
    /// Create a new SecurityConfig with default values
    pub fn new() -> Self {
        Self {
            enable_whitelist: default_enable_whitelist(),
        }
    }

    /// Create a SecurityConfig with whitelist enabled/disabled
    pub fn with_whitelist(enable_whitelist: bool) -> Self {
        Self { enable_whitelist }
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
}

// Default functions
fn default_enable_whitelist() -> bool {
    DEFAULT_ENABLE_WHITELIST
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_security_config_creation() {
        let config = SecurityConfig::new();
        assert_eq!(config.enable_whitelist, DEFAULT_ENABLE_WHITELIST);
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
    }
}
