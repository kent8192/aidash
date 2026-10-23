//! Contracts of Node Registrations (#241).
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::Serialize;

pub use aidash_domain::nodes::registration::RegistrationInput;

/// A Node Registration as its Tenant sees it.
#[derive(Serialize, JsonSchema)]
pub(crate) struct RegistrationRecord {
	/// The owning Node's name.
	pub(crate) node: String,
	/// The registered Node's address.
	pub(crate) source_node: String,
	/// Where the other Deployment answers; absent for a Node of this one.
	pub(crate) endpoint: Option<String>,
	/// The pinned Deployment Key's key ID; absent for a Node of this Deployment.
	pub(crate) key_id: Option<String>,
	pub(crate) atomic_transactions: bool,
	pub(crate) enabled: bool,
	pub(crate) revision: i64,
	/// `oidc:<identity>` or `deployment_administrator`.
	pub(crate) actor: String,
	pub(crate) created_at: DateTime<Utc>,
	pub(crate) updated_at: DateTime<Utc>,
}

#[derive(Serialize, JsonSchema)]
#[schemars(rename = "NodeRegistrationRevision")]
pub(crate) struct RegistrationRevision {
	pub(crate) sequence: i64,
	#[serde(flatten)]
	pub(crate) registration: RegistrationRecord,
}

impl From<aidash_domain::nodes::registration::NodeRegistration> for RegistrationRecord {
	fn from(row: aidash_domain::nodes::registration::NodeRegistration) -> Self {
		let (endpoint, _, key_id) = row.counterpart.columns();
		Self {
			endpoint: endpoint.map(str::to_owned),
			key_id: key_id.map(str::to_owned),
			node: row.node.into_string(),
			source_node: row.source_node.to_string(),
			atomic_transactions: row.atomic_transactions,
			enabled: row.enabled,
			revision: row.revision,
			actor: row.actor,
			created_at: row.created_at,
			updated_at: row.updated_at,
		}
	}
}
