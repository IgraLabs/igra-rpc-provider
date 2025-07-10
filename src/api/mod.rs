/// API layer modules
///
/// This module contains HTTP request/response handling logic following clean architecture principles.
/// Business logic is delegated to appropriate services in the service layer.
pub mod rpc;

// Re-export main handler for ease of use
pub use rpc::handle_rpc;
