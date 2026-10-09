//! Business models, authorization rules, and state invariants.
pub mod catalog;
pub mod context;
pub mod decision;
pub mod deployment;
pub mod entities;
pub mod exposure;
pub mod identity;
pub mod media;
pub mod memory;
pub mod model;
pub mod policy;
pub mod provider;
pub mod run_input;
pub mod run_state;
pub mod semantic;

pub use entities::*;

/// An invariant rejected before any external effect is performed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
	#[error("{0}")]
	Invalid(String),
	#[error("{0}")]
	Conflict(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<serde_json::Error> for Error {
	fn from(error: serde_json::Error) -> Self {
		Self::Invalid(error.to_string())
	}
}

pub mod registry;
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
