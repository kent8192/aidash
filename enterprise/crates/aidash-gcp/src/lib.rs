//! Google Cloud adapters that only the Cloud and Dedicated Offerings use.
#[cfg(feature = "cloud")]
pub mod capability;
pub mod gcip;
#[cfg(feature = "cloud")]
pub mod secret_manager;
