//! Node Registrations (ADR 0064): a Tenant Administrator's decision that one
//! of its Nodes accepts one named Node of another Tenant, and addresses
//! outbound exchange to it. The record pins what the receiving Node needs to
//! check that Node's tokens.
use super::{DeploymentId, DeploymentKeyId, NodeAddress, NodeName};
use crate::configuration::validate_peer_endpoint;
use crate::tenancy::TenantName;
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::Deserialize;

/// The pinned Deployment Key of another Deployment. `public_key_pem` hashes to
/// `key_id`, which a Tenant Administrator typed from the other side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedKey {
	pub key_id: DeploymentKeyId,
	pub public_key_pem: String,
}

/// What a Node Registration knows about the registered Node's side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Counterpart {
	/// A Node of this Deployment: in-process exchange pins no key.
	SameDeployment,
	OtherDeployment {
		endpoint: String,
		key: PinnedKey,
	},
}

impl Counterpart {
	/// Reads the three nullable columns together. The table's check keeps them
	/// all set or all null, and so does this.
	pub fn from_columns(
		endpoint: Option<String>,
		public_key_pem: Option<String>,
		key_id: Option<String>,
	) -> Result<Self> {
		match (endpoint, public_key_pem, key_id) {
			(None, None, None) => Ok(Self::SameDeployment),
			(Some(endpoint), Some(public_key_pem), Some(key_id)) => Ok(Self::OtherDeployment {
				endpoint,
				key: PinnedKey {
					key_id: DeploymentKeyId::parse(&key_id)?,
					public_key_pem,
				},
			}),
			_ => Err(Error::Invalid(
				"a Node Registration pins an endpoint, a public key and its key ID together".into(),
			)),
		}
	}

	/// The `source_endpoint`, `source_public_key_pem` and `source_key_id` columns.
	pub fn columns(&self) -> (Option<&str>, Option<&str>, Option<&str>) {
		match self {
			Self::SameDeployment => (None, None, None),
			Self::OtherDeployment { endpoint, key } => (
				Some(endpoint),
				Some(&key.public_key_pem),
				Some(key.key_id.as_str()),
			),
		}
	}
}

/// A Tenant Administrator's request to register a Node.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegistrationInput {
	/// The registered Node's address.
	#[schemars(with = "String")]
	pub source_node: NodeAddress,
	/// Where the other Deployment answers. Required for a Node of another
	/// Deployment, absent for a Node of this one.
	#[serde(default)]
	pub endpoint: Option<String>,
	/// The other Deployment Key's key ID, as that Deployment publishes it.
	/// Required exactly when `endpoint` is.
	#[serde(default)]
	#[schemars(with = "Option<String>")]
	pub key_id: Option<DeploymentKeyId>,
	/// Whether the Node may take part in an atomic transaction with the
	/// owning Node.
	pub atomic_transactions: bool,
	pub enabled: bool,
	/// Zero to create; the revision being replaced otherwise.
	pub expected_revision: i64,
}

/// What a valid request asks for, before the other Deployment is asked
/// anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Requested {
	SameDeployment,
	OtherDeployment {
		endpoint: String,
		key_id: DeploymentKeyId,
	},
	/// A request that disables a registration: it needs no answer from the
	/// other Deployment, which may be the very reason for disabling, and keeps
	/// what the registration pinned.
	Keep,
}

impl RegistrationInput {
	/// Checks the request against `owner`, the Node that would hold the
	/// registration, in the Deployment `own`.
	pub fn requested(&self, own: &DeploymentId, owner: &NodeName) -> Result<Requested> {
		if !(0..i64::MAX).contains(&self.expected_revision) {
			return Err(Error::Invalid("invalid Node Registration revision".into()));
		}
		if self.source_node.deployment == *own && self.source_node.node == *owner {
			return Err(Error::Invalid(
				"a Node never registers itself; its Tenant's policy decides within the Tenant"
					.into(),
			));
		}
		if !self.enabled {
			return if self.expected_revision == 0 {
				Err(Error::Invalid(
					"disabling needs an existing Node Registration".into(),
				))
			} else {
				Ok(Requested::Keep)
			};
		}
		if self.source_node.deployment == *own {
			if self.endpoint.is_some() || self.key_id.is_some() {
				return Err(Error::Invalid(
					"a Node of this Deployment has no endpoint or key ID".into(),
				));
			}
			return Ok(Requested::SameDeployment);
		}
		let (Some(endpoint), Some(key_id)) = (&self.endpoint, &self.key_id) else {
			return Err(Error::Invalid(
				"a Node of another Deployment needs its endpoint and key ID".into(),
			));
		};
		validate_peer_endpoint(endpoint)?;
		Ok(Requested::OtherDeployment {
			endpoint: endpoint.trim_end_matches('/').to_owned(),
			key_id: key_id.clone(),
		})
	}
}

/// A Node Registration as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRegistration {
	/// The owning Node's Tenant.
	pub tenant: TenantName,
	/// The owning Node.
	pub node: NodeName,
	/// The registered Node.
	pub source_node: NodeAddress,
	pub counterpart: Counterpart,
	pub atomic_transactions: bool,
	pub enabled: bool,
	pub revision: i64,
	pub actor: String,
	pub created_at: DateTime<Utc>,
	pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests;
