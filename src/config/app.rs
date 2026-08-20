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
    /// Wallet connection configuration.
    ///
    /// `#[serde(default)]` so a read-only deployment can omit `[wallet]` entirely — the section is
    /// submission-only and `validate_config` skips its validation when `security.read_only` is set.
    /// The default is empty strings, which read-write deployments still fail on in
    /// `WalletConfig::validate`. Note this is deliberately a *field* default, not a container default
    /// on `WalletConfig`: a partially specified `[wallet]` must still be a hard deserialization error
    /// rather than silently defaulting a mistyped key to an empty string.
    #[serde(default)]
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
    ///
    /// Wallet and lane settings govern transaction *submission* only. A read-only deployment cannot
    /// submit — `api::routing` rejects every write method — so requiring them there would force
    /// operators to invent values nothing ever reads. Both are therefore skipped when
    /// `security.read_only` is set. Every other domain is validated unconditionally, and the order is
    /// unchanged so read-write error precedence is identical to before.
    fn validate_config(config: &AppConfig) -> Result<(), AppError> {
        let read_only = config.security.is_read_only();
        if read_only {
            info!("Read-only mode: skipping wallet and lane configuration validation");
        }

        // Validate each domain configuration
        config
            .server
            .validate()
            .map_err(|e| AppError::ConfigError(format!("Server config: {e}")))?;

        config
            .proxy
            .validate()
            .map_err(|e| AppError::ConfigError(format!("Proxy config: {e}")))?;

        if !read_only {
            config
                .wallet
                .validate()
                .map_err(|e| AppError::ConfigError(format!("Wallet config: {e}")))?;
        }

        config
            .security
            .validate()
            .map_err(|e| AppError::ConfigError(format!("Security config: {e}")))?;

        config
            .mining
            .validate()
            .map_err(|e| AppError::ConfigError(format!("Mining config: {e}")))?;

        if !read_only {
            config
                .lane
                .validate()
                .map_err(|e| AppError::ConfigError(format!("Lane config: {e}")))?;
        }

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

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_KASPA_ADDRESS: &str =
        "kaspatest:qpv8hxvmtvu0tjruup8y5ggqnx9qt5cre32vxrk8073v28w94g99xt57cy60h";

    /// Build a config that is valid in every domain except wallet and lane, which callers set to
    /// exercise the read-only skips. Built structurally rather than through `AppConfig::load`, which
    /// requires a `config.toml` in the process working directory.
    fn base_config(read_only: bool) -> AppConfig {
        AppConfig {
            server: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 8535,
            },
            proxy: ProxyConfig::with_el_url("http://localhost:12345".to_string()),
            wallet: WalletConfig::default(),
            security: SecurityConfig {
                enable_whitelist: true,
                read_only,
            },
            mining: MiningConfig::default(),
            lane: LaneConfig::default(),
            gas: GasConfig::default(),
            retry: RetryConfig::default(),
        }
    }

    fn valid_wallet() -> WalletConfig {
        WalletConfig::new(
            "http://localhost:8082".to_string(),
            VALID_KASPA_ADDRESS.to_string(),
        )
    }

    fn expect_config_error(result: Result<(), AppError>) -> String {
        match result {
            Ok(()) => panic!("expected validation to fail, but it succeeded"),
            Err(AppError::ConfigError(message)) => message,
            Err(other) => panic!("expected AppError::ConfigError, got {other:?}"),
        }
    }

    /// The regression this ticket exists for: a read-only deployment with no wallet settings and no
    /// lane id must validate. Both would fail in read-write mode.
    #[test]
    fn read_only_skips_wallet_and_lane_validation() {
        let config = base_config(true);

        assert!(
            AppConfig::validate_config(&config).is_ok(),
            "read-only config with empty wallet and no lane id should validate"
        );
    }

    /// Negative control: the same config in read-write mode must still be rejected, and wallet is
    /// reported first because it validates before lane.
    #[test]
    fn read_write_still_requires_wallet() {
        let config = base_config(false);

        let message = expect_config_error(AppConfig::validate_config(&config));
        assert!(
            message.contains("Wallet config"),
            "expected a wallet error, got: {message}"
        );
    }

    /// Negative control: a read-write deployment with a valid wallet but no lane id must still be
    /// rejected by lane validation.
    #[test]
    fn read_write_still_requires_lane_id() {
        let mut config = base_config(false);
        config.wallet = valid_wallet();

        let message = expect_config_error(AppConfig::validate_config(&config));
        assert!(
            message.contains("Lane config") && message.contains("IGRA_LANE_ID"),
            "expected a lane error naming IGRA_LANE_ID, got: {message}"
        );
    }

    /// A fully specified read-write config keeps validating, so existing deployments are unaffected.
    #[test]
    fn read_write_with_full_config_validates() {
        let mut config = base_config(false);
        config.wallet = valid_wallet();
        config.lane = LaneConfig::disabled();

        assert!(
            AppConfig::validate_config(&config).is_ok(),
            "a fully specified read-write config should still validate"
        );
    }

    /// Read-only mode ignores wallet values entirely rather than half-validating them; a malformed
    /// URI that nothing ever dials must not block startup.
    #[test]
    fn read_only_ignores_malformed_wallet_values() {
        let mut config = base_config(true);
        config.wallet = WalletConfig::new("not-a-uri".to_string(), "not-an-address".to_string());

        assert!(
            AppConfig::validate_config(&config).is_ok(),
            "read-only mode should not validate inert wallet values"
        );
    }

    // ---- Deserialization ----
    //
    // The tests above build `AppConfig` structurally and so never exercise serde. These pin the
    // `#[serde(default)]` on the `wallet` field: without it, a read-only config that omits `[wallet]`
    // fails to deserialize before `validate_config` ever gets a chance to skip it.

    fn deserialize(toml: &str) -> Result<AppConfig, config::ConfigError> {
        Config::builder()
            .add_source(config::File::from_str(toml, config::FileFormat::Toml))
            .build()?
            .try_deserialize::<AppConfig>()
    }

    fn config_without_wallet_section(read_only: bool) -> String {
        format!(
            r#"
            [server]
            host = "127.0.0.1"
            port = 8535

            [proxy]
            el_url = "http://localhost:12345"

            [security]
            enable_whitelist = true
            read_only = {read_only}

            [mining]
            tx_id_prefix = [0x97, 0xb1]
            "#
        )
    }

    #[test]
    fn read_only_config_may_omit_wallet_section() {
        let config = deserialize(&config_without_wallet_section(true))
            .expect("read-only config omitting [wallet] should deserialize");

        assert_eq!(config.wallet.wallet_daemon_uri, "");
        assert!(
            AppConfig::validate_config(&config).is_ok(),
            "read-only config omitting [wallet] should validate"
        );
    }

    /// Omitting `[wallet]` is only tolerated because read-only never reads it. A read-write
    /// deployment that omits it still fails — now naming the missing setting instead of the missing
    /// section.
    #[test]
    fn read_write_config_omitting_wallet_section_still_fails() {
        let config = deserialize(&config_without_wallet_section(false))
            .expect("omitting [wallet] should deserialize regardless of mode");

        let message = expect_config_error(AppConfig::validate_config(&config));
        assert!(
            message.contains("Wallet daemon URI cannot be empty"),
            "expected the empty-URI error, got: {message}"
        );
    }

    /// The field default must not become a container default: a `[wallet]` section that is present
    /// but incomplete stays a hard error, so a mistyped key cannot silently become an empty string.
    #[test]
    fn partial_wallet_section_is_still_a_deserialization_error() {
        let toml = r#"
            [server]
            host = "127.0.0.1"
            port = 8535

            [proxy]
            el_url = "http://localhost:12345"

            [wallet]
            wallet_daemon_uri = "http://localhost:8082"

            [security]
            enable_whitelist = true
            read_only = true

            [mining]
            tx_id_prefix = [0x97, 0xb1]
        "#;

        assert!(
            deserialize(toml).is_err(),
            "a partially specified [wallet] section must not deserialize"
        );
    }
}
