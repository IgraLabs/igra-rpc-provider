mod api;
mod clients;
mod config;
mod error;
mod services;
mod types;

use axum::{routing::post, Router};
use config::AppConfig;
use std::net::SocketAddr;
use tracing::{debug, info};

#[tokio::main]
async fn main() {
    // Initialize tracing for logging
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Load the application configuration
    let config = AppConfig::load();

    // Parse and create a socket address from the configuration
    let addr = SocketAddr::from((
        config.server.host.parse::<std::net::IpAddr>().unwrap(),
        config.server.port,
    ));

    debug!(?config, "Config");
    info!(%addr, "IGRA RPC provider is running");

    // Build the Axum router
    let app = Router::new()
        .route("/", post(api::rpc::handle_rpc))
        .with_state(config);

    // Bind the listener to the specified address
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind to address");

    // Start the server using the Axum framework
    axum::serve(listener, app.into_make_service())
        .await
        .expect("Server failed");
}
