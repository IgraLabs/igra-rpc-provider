use crate::error::AppError;
use config::{Config, File};
use serde::Deserialize;
use std::env;
use tracing::{debug, info};

// Re-export domain-specific configurations
pub use super::{
    validate_all_configs, ConfigValidation, GasConfig, LaneConfig, MiningConfig, ProxyConfig,
    RetryConfig, SecurityConfig, ServerConfig, WalletConfig,
};

/// Main application configuration that composes all domain-specific configurations
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    /// HTTP server configuration
    pub server: ServerConfig,
    /// EL proxy configuration (replaces old ElConfig)
    #[serde(alias = "el")]
    pub proxy: ProxyConfig,
    /// Wallet connection configuration
    pub wallet: WalletConfig,
    /// Security and whitelist configuration
    pub security: SecurityConfig,
    /// Mining configuration
    pub mining: MiningConfig,
    /// KIP-21 IGRA lane enforcement configuration
    #[serde(default)]
    pub lane: LaneConfig,
    /// Gas pricing configuration
    #[serde(default)]
    pub gas: GasConfig,
    /// Retry configuration for transient errors
    #[serde(default)]
    pub retry: RetryConfig,
}

/// Legacy ElConfig for backward compatibility during transition
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ElConfig {
    pub url: String,
}

// Convert ElConfig to ProxyConfig for backward compatibility
impl From<ElConfig> for ProxyConfig {
    fn from(el_config: ElConfig) -> Self {
        ProxyConfig::with_el_url(el_config.url)
    }
}

impl AppConfig {
    /// Load application configuration from file and environment variables
    pub fn load() -> Result<Self, AppError> {
        let env_mappings = [
            // Server configuration
            ("SERVER_HOST", "server.host"),
            ("SERVER_PORT", "server.port"),
            // Proxy configuration (backward compatibility)
            ("EL_URL", "proxy.el_url"),
            ("EL_WS_URL", "proxy.el_ws_url"),
            ("PROXY_TIMEOUT_SECONDS", "proxy.timeout_seconds"),
            ("PROXY_MAX_RETRIES", "proxy.max_retries"),
            ("PROXY_RETRY_DELAY_MS", "proxy.retry_delay_ms"),
            // Wallet configuration
            ("WALLET_DAEMON_URI", "wallet.wallet_daemon_uri"),
            ("WALLET_TO_ADDRESS", "wallet.to_address"),
            // Security configuration
            ("SECURITY_ENABLE_WHITELIST", "security.enable_whitelist"),
            ("READ_ONLY", "security.read_only"),
            // Mining configuration
            ("TX_ID_PREFIX", "mining.tx_id_prefix"),
            ("MINING_TIMEOUT_SECONDS", "mining.timeout_seconds"),
            // KIP-21 IGRA lane configuration
            ("IGRA_LANE_ID", "lane.lane_id"),
            ("LANE_ENFORCEMENT_DISABLED", "lane.enforcement_disabled"),
            // Gas configuration
            (
                "MIN_PROTOCOL_FEE_PER_GAS_GWEI",
                "gas.min_protocol_fee_per_gas_gwei",
            ),
            // Retry configuration
            ("RETRY_MAX_ATTEMPTS", "retry.max_attempts"),
            ("RETRY_INITIAL_DELAY_MS", "retry.initial_delay_ms"),
            ("RETRY_MAX_DELAY_MS", "retry.max_delay_ms"),
        ];

        let mut builder = Config::builder().add_source(File::with_name("config").required(true));

        for (env_var, config_path) in env_mappings {
            if let Ok(value) = env::var(env_var) {
                debug!("Overriding {} with value: {}", config_path, &value);
                builder = builder
                    .set_override(config_path, value)
                    .map_err(|e| AppError::ConfigError(e.to_string()))?;
            }
        }

        let config = builder
            .build()
            .map_err(|e| AppError::ConfigError(e.to_string()))?
            .try_deserialize::<AppConfig>()
            .map_err(|e| AppError::ConfigError(e.to_string()))?;

        // Validate all domain-specific configurations
        Self::validate_config(&config)?;

        info!("Loaded config: {:?}", config);
        Ok(config)
    }

    /// Validate all domain-specific configurations
    fn validate_config(config: &AppConfig) -> Result<(), AppError> {
        // Validate each domain configuration
        config
            .server
            .validate()
            .map_err(|e| AppError::ConfigError(format!("Server config: {e}")))?;

        config
            .proxy
            .validate()
            .map_err(|e| AppError::ConfigError(format!("Proxy config: {e}")))?;

        config
            .wallet
            .validate()
            .map_err(|e| AppError::ConfigError(format!("Wallet config: {e}")))?;

        config
            .security
            .validate()
            .map_err(|e| AppError::ConfigError(format!("Security config: {e}")))?;

        config
            .mining
            .validate()
            .map_err(|e| AppError::ConfigError(format!("Mining config: {e}")))?;

        config
            .lane
            .validate()
            .map_err(|e| AppError::ConfigError(format!("Lane config: {e}")))?;

        config
            .gas
            .validate()
            .map_err(|e| AppError::ConfigError(format!("Gas config: {e}")))?;

        config
            .retry
            .validate()
            .map_err(|e| AppError::ConfigError(format!("Retry config: {e}")))?;

        Ok(())
    }

    /// Get the EL URL for backward compatibility
    pub fn el_url(&self) -> &str {
        self.proxy.el_url()
    }

    /// Convert to legacy ElConfig for backward compatibility
    pub fn to_el_config(&self) -> ElConfig {
        ElConfig {
            url: self.proxy.el_url().to_string(),
        }
    }
}
