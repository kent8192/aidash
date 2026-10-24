//! One full-stack Reinhardt Pages Marketplace project.
pub mod apps;
#[cfg(client)]
pub mod client;
pub mod config;
#[cfg(client)]
pub(crate) use apps::distribution::{
	client::components::{app, publishers, ui},
	services::client::api,
};
#[cfg(server)]
pub use apps::distribution::{
	server::migrations,
	services::server::{Error, Marketplace, MarketplaceHandler, Result},
};
#[cfg(server)]
pub(crate) use apps::distribution::{
	server::{auth, store},
	services::server::json,
};
#[cfg(server)]
pub use config::settings::Config;
