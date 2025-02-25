use config::{Config, Environment, File};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub el: ElConfig,
    pub wallet: WalletConfig,
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
    pub command: String,
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
