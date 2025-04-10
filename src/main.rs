mod api;
mod clients;
mod config;
mod error;
mod services;
mod types;

use axum::{routing::post, Router};
use config::AppConfig;
use services::transaction::{TransactionRequest, start_transaction_processor};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

// Define a type for our shared state
pub struct AppState {
    pub config: AppConfig,
    pub transaction_sender: mpsc::Sender<TransactionRequest>,
}

#[tokio::main]
async fn main() {
    // Initialize enhanced tracing for logging
    setup_logging();

    // Load the application configuration
    let config = AppConfig::load();

    // Parse and create a socket address from the configuration
    let addr = SocketAddr::from((
        config.server.host.parse::<std::net::IpAddr>().unwrap(),
        config.server.port,
    ));

    debug!(?config, "Config loaded");
    info!("IGRA RPC PROVIDER STARTING");
    info!("Listening on: {}", addr);
    info!("EL client URL: {}", config.el.url);
    info!("KASPA wallet: {}", config.wallet.wallet_daemon_uri);

    // Start the transaction processor and get the sender
    let transaction_sender = start_transaction_processor(config.clone());
    info!("Transaction processor started");

    // Set up the shared state
    let state = Arc::new(AppState {
        config,
        transaction_sender,
    });

    // Build the Axum router
    let app = Router::new()
        .route("/", post(api::rpc::handle_rpc))
        .with_state(state);

    info!("Router configured, starting server...");

    // Bind the listener to the specified address
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind to address");

    info!("Server started, ready to accept connections");

    // Start the server using the Axum framework
    axum::serve(listener, app.into_make_service())
        .await
        .expect("Server failed");
}

/// Sets up comprehensive logging
fn setup_logging() {
    // Default to INFO level but allow override via env var
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| {
            EnvFilter::new("info")
                .add_directive("igra_rpc_provider=debug".parse().unwrap())
                .add_directive("tower_http=debug".parse().unwrap())
        });

    // Create and register the subscriber with console output only
    tracing_subscriber::registry()
        .with(fmt::layer()
            .with_ansi(true)
            .with_target(true)
            .with_thread_ids(true))
        .with(env_filter)
        .init();

    info!("Logging initialized");
}
