use crate::error::AppError;
use config::{Config, File};
use serde::Deserialize;
use std::env;
use tracing::{debug, info};

const DEFAULT_ENABLE_WHITELIST: bool = true;
const DEFAULT_REQUIRED_PREFIX: &[u8] = &[0x97, 0xb1];
const DEFAULT_TIMEOUT_SECONDS: u64 = 10;
const DEFAULT_MIN_BASE_FEE_GWEI: u64 = 100;
const HASH_SIZE: usize = 32;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub el: ElConfig,
    pub wallet: WalletConfig,
    pub security: SecurityConfig,
    pub mining: MiningConfig,
    #[serde(default)]
    pub gas: GasConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ElConfig {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WalletConfig {
    pub wallet_daemon_uri: String,
    pub to_address: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SecurityConfig {
    #[serde(default = "default_enable_whitelist")]
    pub enable_whitelist: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MiningConfig {
    #[serde(default = "default_required_prefix")]
    pub required_prefix: Vec<u8>,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GasConfig {
    #[serde(default = "default_min_base_fee_gwei")]
    pub min_base_fee_gwei: u64,
}

impl Default for MiningConfig {
    fn default() -> Self {
        Self {
            required_prefix: DEFAULT_REQUIRED_PREFIX.to_vec(),
            timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
        }
    }
}

impl MiningConfig {
    /// Validates the mining configuration parameters
    pub fn validate(&self) -> Result<(), AppError> {
        // Validate required_prefix length (empty prefix disabled, max 32 bytes for Kaspa hashes)
        if self.required_prefix.is_empty() {
            return Err(AppError::ConfigError(
                "Mining required_prefix cannot be empty".to_string(),
            ));
        }

        if self.required_prefix.len() > HASH_SIZE {
            return Err(AppError::ConfigError(format!(
                "Mining required_prefix cannot exceed {} bytes (Kaspa hash size), got {} bytes",
                HASH_SIZE,
                self.required_prefix.len()
            )));
        }

        // Validate timeout is reasonable (<300 seconds)
        if self.timeout_seconds == 0 || self.timeout_seconds > 300 {
            return Err(AppError::ConfigError(format!(
                "Mining timeout_seconds must be less than 300 seconds, got {} seconds",
                self.timeout_seconds
            )));
        }

        Ok(())
    }
}

impl Default for GasConfig {
    fn default() -> Self {
        Self {
            min_base_fee_gwei: default_min_base_fee_gwei(),
        }
    }
}

fn default_enable_whitelist() -> bool {
    DEFAULT_ENABLE_WHITELIST
}

fn default_required_prefix() -> Vec<u8> {
    DEFAULT_REQUIRED_PREFIX.to_vec()
}

fn default_timeout_seconds() -> u64 {
    DEFAULT_TIMEOUT_SECONDS
}

fn default_min_base_fee_gwei() -> u64 {
    DEFAULT_MIN_BASE_FEE_GWEI
}

impl AppConfig {
    pub fn load() -> Result<Self, AppError> {
        let env_mappings = [
            ("SERVER_HOST", "server.host"),
            ("SERVER_PORT", "server.port"),
            ("EL_URL", "el.url"),
            ("WALLET_DAEMON_URI", "wallet.wallet_daemon_uri"),
            ("WALLET_TO_ADDRESS", "wallet.to_address"),
            ("SECURITY_ENABLE_WHITELIST", "security.enable_whitelist"),
            ("MINING_REQUIRED_PREFIX", "mining.required_prefix"),
            ("MINING_TIMEOUT_SECONDS", "mining.timeout_seconds"),
            ("GAS_MIN_BASE_FEE_GWEI", "gas.min_base_fee_gwei"),
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

        // Validate mining configuration
        config.mining.validate()?;

        info!("Loaded config: {:?}", config);
        Ok(config)
    }
}
