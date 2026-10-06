//! Explicit Home-owned generation intent; no local execution is fabricated.
use crate::{Task, generation::remote::Ancestor};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "RemoteGenerationIntent")]
pub struct Intent {
	pub id: Uuid,
	pub home_node: String,
	pub source_tenant: String,
	pub source_subject: String,
	pub task: Task,
	pub target_node: String,
	pub policy_id: String,
	pub policy_revision: i64,
	pub lineage: Vec<Ancestor>,
	pub reason: String,
	pub ttl_seconds: i64,
	pub expires_at: DateTime<Utc>,
}

pub mod guards;

use crate::registry::EntityRef;
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "RemoteGenerationInput")]
pub struct Input {
	pub id: Uuid,
	pub node_id: String,
	pub policy_id: String,
	pub policy_revision: i64,
	pub ttl_seconds: i64,
	pub reason: String,
}
#[derive(Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "RemoteGenerationPrepared")]
pub struct Prepared {
	pub intent_id: Uuid,
	pub node_id: String,
	pub request_id: Uuid,
	pub agent: EntityRef,
	pub status: String,
	pub prepared: bool,
	pub expires_at: DateTime<Utc>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
	pub intent_id: Uuid,
}

impl Input {
	pub fn validate(&self, home_node: &str) -> crate::Result<()> {
		crate::configuration::validate_node_id(&self.node_id)?;
		if self.id.is_nil()
			|| self.node_id == home_node
			|| self.policy_revision < 1
			|| !(1..=3600).contains(&self.ttl_seconds)
			|| self.reason.trim().is_empty()
			|| self.reason.len() > 4096
		{
			return Err(crate::Error::Invalid(
				"invalid remote generation intent".into(),
			));
		}
		Ok(())
	}
}
