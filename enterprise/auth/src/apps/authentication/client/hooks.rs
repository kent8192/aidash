//! Authentication state and the lifetime of the upstream browser SDK.
mod authentication;
mod forms;
pub use authentication::{Action, App, Factor, State, use_authentication};
