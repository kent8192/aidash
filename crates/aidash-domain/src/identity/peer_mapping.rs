//! Explicit peer mappings bind a local credential to a distinct remote subject.
//! A Peer Mapping belongs to one receiving Node and one registered sending
//! Node: the same sending subject may be mapped at several receiving Nodes,
//! and the sending Tenant is never part of the key (#241).
use crate::{
	Error, Result,
	nodes::{DeploymentId, NodeAddress, NodeName},
	policy::identifier,
};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PeerMappingInput {
	/// The receiving Node of the mapping's Tenant.
	#[schemars(with = "String")]
	pub node: NodeName,
	/// The registered sending Node.
	#[schemars(with = "String")]
	pub source_node: NodeAddress,
	pub source_subject: String,
	pub credential_id: Uuid,
	pub enabled: bool,
	pub expected_revision: i64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Mapping {
	pub node: NodeName,
	pub source_node: NodeAddress,
	pub source_subject: String,
	pub tenant: String,
	pub credential_id: Uuid,
	pub enabled: bool,
	pub revision: i64,
	pub actor: String,
	pub updated_at: DateTime<Utc>,
}
impl PeerMappingInput {
	/// A Node never maps itself, and a mapping needs a subject and a
	/// non-negative revision.
	pub fn validate(&self, tenant: &str, own: &DeploymentId) -> Result<()> {
		identifier(tenant)?;
		identifier(&self.source_subject)?;
		if (self.source_node.deployment == *own && self.source_node.node == self.node)
			|| !(0..i64::MAX).contains(&self.expected_revision)
		{
			return Err(Error::Invalid("invalid peer mapping or revision".into()));
		}
		Ok(())
	}
	pub fn require_existing_revocation(&self) -> Result<()> {
		if !self.enabled && self.expected_revision == 0 {
			return Err(Error::Invalid(
				"revocation requires an existing mapping".into(),
			));
		}
		Ok(())
	}
}
#[cfg(test)]
mod tests;
