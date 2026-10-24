//! Authentication state and the lifetime of the upstream browser SDK.
mod authentication;
pub use authentication::{Action, App, Factor, State, use_authentication};
