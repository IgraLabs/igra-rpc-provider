use crate::error::AppError;
use config::{Config, File};
use serde::Deserialize;
use std::env;
use tracing::debug;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub el: ElConfig,
    pub wallet: WalletConfig,
    pub security: SecurityConfig,
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

fn default_enable_whitelist() -> bool {
    true
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
            .try_deserialize()
            .map_err(|e| AppError::ConfigError(e.to_string()))?;

        debug!("Loaded config: {:?}", config);
        Ok(config)
    }
}
