//! IGRA RPC Provider Library
//!
//! Core library components for the IGRA RPC Provider

use crate::clients::wallet_caller::WalletCaller;
use crate::config::AppConfig;
use crate::services::{proxy::ProxyService, transaction::TransactionRequest};
use std::sync::Arc;
use tokio::sync::mpsc;

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
}

impl AppState {
    /// Create new application state
    pub fn new(
        config: AppConfig,
        transaction_sender: mpsc::Sender<TransactionRequest>,
        wallet_caller: Arc<WalletCaller>,
        proxy_service: ProxyService,
    ) -> Self {
        Self {
            config,
            transaction_sender,
            wallet_caller,
            proxy_service,
        }
    }
}
