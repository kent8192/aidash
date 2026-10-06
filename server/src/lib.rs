//! Aidash backend, implemented by Reinhardt applications.
extern crate self as aidash_server;

pub mod apps;
pub mod config;
pub mod error;
pub mod http;
pub(crate) mod response;
pub use apps::execution::context;
pub use apps::execution::repositories::store;
pub use apps::execution::{activation, capabilities, generation};
pub use apps::execution::{bus, openrouter, provider, runtime as harness, web_search};
pub use apps::federation::remote::runtime as federation;
pub use apps::federation::transactions;
pub use apps::identity as authorization;
pub use apps::identity::oidc as dashboard_auth;
pub use apps::knowledge as semantic;
pub use apps::marketplace::services as marketplace;
pub use apps::operations as orchestration;
pub use apps::registry::{self, workbench};
pub use apps::registry::{knowledge, skill_import};
pub use apps::workspaces as collaboration;
pub use apps::workspaces::serializers::entities as domain;
pub use apps::workspaces::sse;
pub use config::settings::get_settings;
pub use config::urls::routes;
pub use error::{Error, Result};

pub use apps::execution::services::lifecycle;

pub use apps::execution::services::capabilities as tool;

pub(crate) mod api_schema;

pub mod bootstrap;

pub mod database;
