//! Application registry for aidash_auth_backend
//!
//! This file maintains the list of installed apps.
//! New apps created with `startapp` will be automatically added here.
pub mod authentication;
pub use authentication::AuthenticationConfig;
