//! IGRA RPC Provider Library
//!
//! Core library components for the IGRA RPC Provider

use crate::clients::wallet_caller::WalletCaller;
use crate::config::AppConfig;
use crate::services::{
    gas_price::GasPriceService, proxy::ProxyService, transaction::TransactionRequest,
};
use std::sync::Arc;
use tokio::sync::{mpsc, Semaphore};

pub mod api;
pub mod clients;
pub mod config;
pub mod error;
pub mod errors;
pub mod services;
pub mod tools;
pub mod types;

/// Shared application state
pub struct AppState {
    /// Application configuration
    pub config: AppConfig,
    /// Channel for transaction processing
    pub transaction_sender: mpsc::Sender<TransactionRequest>,
    /// Wallet caller for Kaspa wallet operations
    pub wallet_caller: Arc<WalletCaller>,
    /// Proxy service for EL client communication
    pub proxy_service: ProxyService,
    /// Shared gas price service (1s-cached effective base fee) used by the synchronous
    /// `eth_sendRawTransaction` accept path for the fee-floor check. Cloned from the proxy's
    /// instance so both share one cache.
    pub gas_price_service: GasPriceService,
    /// Semaphore limiting concurrent WebSocket connections
    pub ws_semaphore: Arc<Semaphore>,
}

impl AppState {
    /// Create new application state
    pub fn new(
        config: AppConfig,
        transaction_sender: mpsc::Sender<TransactionRequest>,
        wallet_caller: Arc<WalletCaller>,
        proxy_service: ProxyService,
        gas_price_service: GasPriceService,
        ws_semaphore: Arc<Semaphore>,
    ) -> Self {
        Self {
            config,
            transaction_sender,
            wallet_caller,
            proxy_service,
            gas_price_service,
            ws_semaphore,
        }
    }
}
