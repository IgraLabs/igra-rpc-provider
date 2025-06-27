//! IGRA RPC Provider Library
//!
//! Core library components for the IGRA RPC Provider

use crate::clients::wallet_caller::WalletCaller;
use crate::services::transaction::TransactionRequest;
use config::AppConfig;
use std::sync::Arc;
use tokio::sync::mpsc;

pub mod api;
pub mod clients;
pub mod config;
pub mod error;
pub mod services;
pub mod types;

/// Shared application state
pub struct AppState {
    pub config: AppConfig,
    pub transaction_sender: mpsc::Sender<TransactionRequest>,
    pub wallet_caller: Arc<WalletCaller>,
}
