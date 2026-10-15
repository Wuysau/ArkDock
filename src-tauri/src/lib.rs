pub mod agent;
pub mod context;
#[cfg(any(feature = "desktop", test))]
pub(crate) mod context_pairing_store;
#[cfg(feature = "desktop")]
pub mod desktop;
pub mod discovery;
pub mod inspector;
pub mod ipc;
pub mod preview;
pub mod scheduler;
pub mod selection;
pub mod windowing;
pub mod workspace;

/// Windows-only pipe transport for [`crate::context`]; compiled with the desktop
/// binary so the listener lifecycle matches the app process.
#[cfg(all(windows, any(feature = "desktop", test)))]
pub(crate) mod context_pipe;
