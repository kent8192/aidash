//! Business models, authorization rules, and state invariants.
pub mod agent_plugin;
pub mod approval_rules;
pub mod catalog;
pub mod completion;
pub mod context;
pub mod decision;
pub mod deployment;
pub mod entities;
pub mod exposure;
pub mod identity;
pub mod keyless;
pub mod media;
pub mod memory;
pub mod model;
pub mod offering;
pub mod plan_connections;
pub mod policy;
pub mod projection;
pub mod provider;
pub mod provider_credentials;
pub mod retention;
pub mod run_input;
pub mod run_state;
pub mod semantic;
pub mod tenancy;
pub mod tenant_secrets;

pub use entities::*;

/// An invariant rejected before any external effect is performed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
	#[error("{0}")]
	Invalid(String),
	#[error("{0}")]
	Conflict(String),
	/// Persisted state written by a build that knows more than this one. The Run is
	/// intact; a worker with the missing capability can load it.
	#[error("persisted state needs a newer worker: {0}")]
	UnsupportedState(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<serde_json::Error> for Error {
	fn from(error: serde_json::Error) -> Self {
		Self::Invalid(error.to_string())
	}
}

pub mod registry;
pub mod research;
pub mod tool;

pub mod capabilities;
pub mod federation;

pub mod configuration;

pub mod workspaces;

pub mod marketplace;

pub mod generation;

pub mod invocation;

pub mod transactions;

pub mod activation;

pub mod egress;
