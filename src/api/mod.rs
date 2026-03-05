/// API layer modules
///
/// This module contains HTTP request/response handling logic following clean architecture principles.
/// Business logic is delegated to appropriate services in the service layer.
pub mod health;
pub mod routing;
pub mod rpc;
pub mod ws;

// Re-export main handlers for ease of use
pub use health::health_check;
pub use rpc::handle_rpc;
pub use ws::handle_ws_upgrade;
