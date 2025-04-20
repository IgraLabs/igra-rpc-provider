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
        let mut settings = Config::builder()
            .add_source(File::with_name("config"))
            .add_source(
                Environment::default()
                    .separator("_") // Use "_" to allow nested structures
                    .prefix(""),    // Ensure there is no optional prefix
            );

        // @todo Make env vars to overwrite defaults w/o this manual mapping.
        for (key, value) in std::env::vars() {
            let mapped_key = match key.as_str() {
                "SERVER_HOST" => "server.host",
                "SERVER_PORT" => "server.port",
                "EL_URL" => "el.url",
                "WALLET_DAEMON_URI" => "wallet.wallet_daemon_uri",
                "WALLET_TO_ADDRESS" => "wallet.to_address",
                "SECURITY_ENABLE_WHITELIST" => "security.enable_whitelist",
                _ => continue, // Skip irrelevant
            };
            settings = settings.set_override(mapped_key, value).unwrap();
        }

        settings
            .build()
            .unwrap()
            .try_deserialize().expect("Invalid config format")
    }
}
