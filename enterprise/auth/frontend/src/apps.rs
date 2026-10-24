//! Application registry for aidash_auth_frontend
//!
//! This file maintains the list of installed apps.
//! New apps created with `startapp` will be automatically added here.
pub mod account;
#[cfg(not(target_arch = "wasm32"))]
pub use account::AccountConfig;
