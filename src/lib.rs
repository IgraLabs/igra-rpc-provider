//! IGRA RPC Provider Library
//!
//! Core library components for the IGRA RPC Provider

use crate::clients::wallet_caller::WalletCaller;
use crate::config::AppConfig;
use crate::services::{proxy::ProxyService, transaction::TransactionRequest};
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
    pub wallet_caller: Option<Arc<WalletCaller>>,
    /// Proxy service for EL client communication
    pub proxy_service: ProxyService,
    /// Semaphore limiting concurrent WebSocket connections
    pub ws_semaphore: Arc<Semaphore>,
}

impl AppState {
    /// Create new application state
    pub fn new(
        config: AppConfig,
        transaction_sender: mpsc::Sender<TransactionRequest>,
        wallet_caller: Option<Arc<WalletCaller>>,
        proxy_service: ProxyService,
        ws_semaphore: Arc<Semaphore>,
    ) -> Self {
        Self {
            config,
            transaction_sender,
            wallet_caller,
            proxy_service,
            ws_semaphore,
        }
    }
}
