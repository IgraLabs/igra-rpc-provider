use config::{Config, Environment, File};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub el: ElConfig,
    pub wallet: WalletConfig,
    pub security: SecurityConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ElConfig {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WalletConfig {
    pub wallet_daemon_uri: String,
    pub to_address: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SecurityConfig {
    #[serde(default = "default_enable_whitelist")]
    pub enable_whitelist: bool,
}

fn default_enable_whitelist() -> bool {
    true
}

impl AppConfig {
    pub fn load() -> Self {
        let settings = Config::builder()
            .add_source(File::with_name("config"))
            .add_source(
                Environment::default().separator("_"), // for nested keys
            )
            .build()
            .unwrap();

        settings.try_deserialize().expect("Invalid config format")
    }
}
