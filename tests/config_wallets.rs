use igra_rpc_provider::config::AppConfig;

#[test]
fn app_config_parses_additional_wallets_array() {
    let toml = r#"
[server]
host = "127.0.0.1"
port = 8535

[proxy]
el_url = "http://localhost:8545"

[wallet]
wallet_daemon_uri = "http://wallet-1:8082"
to_address = "kaspatest:qpv8hxvmtvu0tjruup8y5ggqnx9qt5cre32vxrk8073v28w94g99xt57cy60h"

[[wallets]]
wallet_daemon_uri = "http://wallet-2:8082"
to_address = "kaspatest:qpv8hxvmtvu0tjruup8y5ggqnx9qt5cre32vxrk8073v28w94g99xt57cy60h"

[[wallets]]
wallet_daemon_uri = "http://wallet-3:8082"
to_address = "kaspatest:qpv8hxvmtvu0tjruup8y5ggqnx9qt5cre32vxrk8073v28w94g99xt57cy60h"

[security]
enable_whitelist = false
read_only = false

[mining]
tx_id_prefix = [0x00]
timeout_seconds = 10
"#;

    let config: AppConfig = toml::from_str(toml).expect("Failed to deserialize AppConfig");

    assert_eq!(config.wallet.wallet_daemon_uri, "http://wallet-1:8082");
    assert_eq!(config.wallets.len(), 2);
    assert_eq!(config.wallets[0].wallet_daemon_uri, "http://wallet-2:8082");
    assert_eq!(config.wallets[1].wallet_daemon_uri, "http://wallet-3:8082");

    assert!(config.wallet.validate().is_ok());
    assert!(config.wallets[0].validate().is_ok());
    assert!(config.wallets[1].validate().is_ok());
}

