// Import everything from the library instead
use axum::{
    routing::{get, post},
    Router,
};
use igra_rpc_provider::{
    api,
    config::AppConfig,
    error::AppError,
    services::{
        gas_price::GasPriceService, proxy::ProxyService,
        transaction::{start_transaction_processor, WalletBackend},
    },
    AppState,
};
use std::net::{IpAddr, SocketAddr};
use std::process;
use std::sync::Arc;
use tracing::{debug, error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> Result<(), AppError> {
    // Initialize enhanced tracing for logging
    setup_logging();

    // Load the application configuration
    let config = match AppConfig::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("Failed to load configuration: {e}");
            process::exit(1);
        }
    };

    // Parse and create a socket address from the configuration
    let ip_addr: IpAddr = match config.server.host.parse() {
        Ok(ip) => ip,
        Err(_) => {
            eprintln!("Invalid IP address for server.host in configuration");
            process::exit(1);
        }
    };
    let addr = SocketAddr::from((ip_addr, config.server.port));

    debug!(?config, "Config loaded");
    info!("IGRA RPC PROVIDER STARTING");
    info!("Listening on: {}", addr);
    info!("EL client URL: {}", config.el_url());

    let gas_price_service = GasPriceService::new(config.gas.clone());

    let mut wallet_configs = Vec::with_capacity(config.wallets.len().saturating_add(1));
    wallet_configs.push(config.wallet.clone());
    wallet_configs.extend(config.wallets.clone());

    let mut wallet_backends = Vec::with_capacity(wallet_configs.len());
    for wallet_config in wallet_configs {
        match WalletBackend::connect(wallet_config).await {
            Ok(backend) => {
                info!("KASPA wallet backend connected: {}", backend.daemon_uri);
                wallet_backends.push(backend);
            }
            Err(err) => {
                error!("Failed to connect to wallet backend: {}", err);
            }
        }
    }

    if wallet_backends.is_empty() {
        error!("No wallet backends available; refusing to start");
        process::exit(1);
    }

    // Start the transaction processor and get the sender
    let transaction_sender =
        start_transaction_processor(config.clone(), wallet_backends, gas_price_service.clone());
    info!("Transaction processor started");

    // Create the new services using dependency injection
    let proxy_service = ProxyService::new(config.el_url().to_string(), gas_price_service);

    // Set up the shared application state
    let state = Arc::new(AppState::new(
        config,
        transaction_sender,
        proxy_service,
    ));

    // Build the Axum router
    let app = Router::new()
        .route("/", post(api::rpc::handle_rpc))
        .route("/health", get(api::health::health_check))
        .with_state(state);

    info!("Router configured, starting server...");

    // Bind the listener to the specified address
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("Failed to bind to address {addr}: {e}");
            process::exit(1);
        }
    };

    info!("Server started, ready to accept connections");

    // Start the server using the Axum framework
    if let Err(e) = axum::serve(listener, app.into_make_service()).await {
        eprintln!("Server failed: {e}");
        process::exit(1);
    }

    Ok(())
}

/// Sets up comprehensive logging
fn setup_logging() {
    // Default to INFO level but allow override via env var
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("info")
            .add_directive(
                "igra_rpc_provider=debug"
                    .parse()
                    .expect("Failed to parse static 'igra_rpc_provider=debug' directive"),
            )
            .add_directive(
                "tower_http=debug"
                    .parse()
                    .expect("Failed to parse static 'tower_http=debug' directive"),
            )
    });

    // Create and register the subscriber with console output only
    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_ansi(true)
                .with_target(true)
                .with_thread_ids(true),
        )
        .with(env_filter)
        .init();

    info!("Logging initialized");
}
