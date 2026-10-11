//! Explicit current-session bindings and receipt-based delivery state.
//! Product names/window titles never grant access to a conversation.
mod adapters;
pub mod assisted;
pub mod bridge;
mod router;

pub use adapters::AgentTransport;
pub use router::AgentRouter;
