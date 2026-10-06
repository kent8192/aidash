//! Explicit peer mappings bind a local credential to a distinct remote subject.
use crate::{Error, Result, configuration::validate_node_id, policy::identifier};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PeerMappingInput {
	pub source_node: String,
	pub source_tenant: String,
	pub source_subject: String,
	pub credential_id: Uuid,
	pub enabled: bool,
	pub expected_revision: i64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Mapping {
	pub source_node: String,
	pub source_tenant: String,
	pub source_subject: String,
	pub tenant: String,
	pub credential_id: Uuid,
	pub enabled: bool,
	pub revision: i64,
	pub actor: String,
	pub updated_at: DateTime<Utc>,
}
impl PeerMappingInput {
	pub fn validate(&self, tenant: &str, node: &str) -> Result<()> {
		identifier(tenant)?;
		identifier(&self.source_tenant)?;
		identifier(&self.source_subject)?;
		validate_node_id(&self.source_node)?;
		if self.source_node == node || !(0..i64::MAX).contains(&self.expected_revision) {
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
