//! Application registry for auth
//!
//! This file maintains the list of installed apps.
//! New apps created with `startapp` will be automatically added here.
pub mod authentication;
#[cfg(server)]
pub use authentication::AuthenticationConfig;
