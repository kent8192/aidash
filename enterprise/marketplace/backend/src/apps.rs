//! Application registry for aidash_marketplace_backend
//!
//! This file maintains the list of installed apps.
//! New apps created with `startapp` will be automatically added here.
pub mod distribution;
pub use distribution::DistributionConfig;
