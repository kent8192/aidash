//! Standalone Marketplace project and its distribution app.
pub mod apps;
pub mod config;
pub use apps::distribution::{Config, Error, Marketplace, MarketplaceHandler, Result, migrations};
pub(crate) use apps::distribution::{auth, json, store};
