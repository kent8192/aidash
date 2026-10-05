//! Authority established by a trusted authentication adapter.
/// This type has no deserializer: request bodies cannot establish authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
	Operator,
	Subject { tenant: String, subject: String },
}

pub mod catalog;

pub mod execution;

pub mod authority;

pub mod commands;

pub mod peer_mapping;

pub mod dashboard;

pub mod desktop;
