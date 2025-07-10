// Legacy modules (kept for backward compatibility during transition)
pub mod entry_transaction;
pub mod gas_price;
pub mod mining;
pub mod proxy;
pub mod transaction;

// New refactored service modules with single responsibility
pub mod gas_manager;
pub mod transaction_processor;
pub mod wallet_service;

// Re-exports for easier access
pub use gas_manager::GasManager;
pub use transaction_processor::{
    start_transaction_processor, TransactionProcessor, TransactionRequest,
};
pub use wallet_service::{
    SendTransactionRequest, WalletService, WalletTransactionResult, WalletTransactionStatus,
};

// Legacy re-exports (maintain API compatibility)
pub use gas_price::GasPriceService;
pub use proxy::ProxyService;
