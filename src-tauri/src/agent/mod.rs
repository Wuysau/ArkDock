//! Explicit current-session bindings and receipt-based delivery state.
//! Product names/window titles never grant access to a conversation.
mod adapters;
pub mod assisted;
pub mod bridge;
/// Credential persistence exists only where a credential is written: the desktop
/// app that owns the save dialog, and the pairing tests.
#[cfg(any(feature = "desktop", test))]
pub(crate) mod pairing;
mod router;

pub use adapters::AgentTransport;
pub use router::AgentRouter;
