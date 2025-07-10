//! IGRA RPC Provider Library
//!
//! Core library components for the IGRA RPC Provider

use crate::clients::wallet_caller::WalletCaller;
use crate::config::AppConfig;
use crate::services::{
    gas_manager::GasManager, proxy::ProxyService, transaction::TransactionRequest,
    transaction_processor::TransactionProcessor, wallet_service::WalletService,
};
use std::sync::Arc;
use tokio::sync::mpsc;

pub mod api;
pub mod clients;
pub mod config;
pub mod error;
pub mod errors;
pub mod services;
pub mod types;

/// Application services container with clean dependency injection
#[derive(Clone)]
pub struct AppServices {
    /// Transaction processing service
    pub transaction_processor: Arc<TransactionProcessor>,
    /// Gas price management service
    pub gas_manager: Arc<GasManager>,
    /// EL proxy service
    pub proxy_service: Arc<ProxyService>,
    /// Wallet communication service
    pub wallet_service: Arc<WalletService>,
}

/// Shared application state with clean service separation
pub struct AppState {
    /// Application configuration
    pub config: AppConfig,
    /// Channel for transaction processing
    pub transaction_sender: mpsc::Sender<TransactionRequest>,
    /// Legacy wallet caller (for backward compatibility)
    pub wallet_caller: Arc<WalletCaller>,
    /// Legacy proxy service (for backward compatibility)
    pub proxy_service: ProxyService,
    /// Clean service container
    pub services: AppServices,
}

impl AppServices {
    /// Create new service container with dependency injection
    pub async fn new(config: &AppConfig) -> Result<Self, crate::error::AppError> {
        // Create gas manager service
        let gas_manager = Arc::new(GasManager::new(config.gas.clone()));

        // Create gas price service for proxy
        let gas_price_service =
            crate::services::gas_price::GasPriceService::new(config.gas.clone());

        // Create proxy service
        let proxy_service = Arc::new(ProxyService::new(
            config.el_url().to_string(),
            gas_price_service,
        ));

        // Create wallet service
        let wallet_service = Arc::new(WalletService::new(config.wallet.clone()).await.map_err(
            |e| crate::error::AppError::Internal(format!("Failed to create wallet service: {}", e)),
        )?);

        // Create transaction processor
        let transaction_processor = Arc::new(TransactionProcessor::new(config.clone()));

        Ok(Self {
            transaction_processor,
            gas_manager,
            proxy_service,
            wallet_service,
        })
    }

    /// Create services for testing with mock dependencies
    /// Note: This is a placeholder implementation for testing
    #[cfg(test)]
    pub fn new_for_testing(_config: AppConfig) -> Self {
        // For proper testing, we would need a mocking framework
        // For now, we'll panic to indicate this needs proper implementation
        panic!("new_for_testing requires proper mocking framework - use real AppServices::new() for now");
    }
}

impl AppState {
    /// Create new application state with dependency injection
    pub async fn new(
        config: AppConfig,
        transaction_sender: mpsc::Sender<TransactionRequest>,
        wallet_caller: Arc<WalletCaller>,
        proxy_service: ProxyService,
    ) -> Result<Self, crate::error::AppError> {
        let services = AppServices::new(&config).await?;

        Ok(Self {
            config,
            transaction_sender,
            wallet_caller,
            proxy_service,
            services,
        })
    }
}
