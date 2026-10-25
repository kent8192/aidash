//! Application registry for marketplace
//!
//! This file maintains the list of installed apps.
//! New apps created with `startapp` will be automatically added here.
pub mod distribution;
#[cfg(server)]
pub use distribution::DistributionConfig;
