//! Origin-owned provider approvals, lineage and idempotent usage contracts.
use crate::registry::rules::digest;
use crate::semantic::{
	Failure,
	remote::{ContractError, Provider, Result},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "GenerationRemoteAllowance")]
pub struct Allowance {
	pub provider: Provider,
	pub calls_per_agent: i64,
	pub call_budget: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "GenerationRemoteApprovals")]
pub struct Approvals {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub embedding: Option<Allowance>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub compaction: Option<Allowance>,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub inference: Vec<Provider>,
	/// Home-owned native memory model roles. The ancestor's total token budget
	/// and the pinned memory workflow's call/token/cost caps both apply.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub memory: Vec<Provider>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "GenerationAncestor")]
pub struct Ancestor {
	pub node_id: String,
	pub tenant: String,
	pub request_id: Uuid,
	pub policy_id: String,
	pub policy_revision: i64,
	pub depth: i32,
	pub expires_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Usage {
	pub operation_id: Uuid,
	pub attempt_id: Uuid,
	pub dispatcher_node: String,
	pub grant_id: Uuid,
	pub admission_id: Uuid,
	pub purpose: Purpose,
	pub provider: Provider,
	pub input_digest: String,
	pub reserved_tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reserved {
	pub owner: Ancestor,
	pub attempt_id: Uuid,
	pub digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
	Embedding,
	Inference,
	Compaction,
	Memory,
}

fn valid_digest(s: &str) -> bool {
	s.strip_prefix("sha256:")
		.is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|c| c.is_ascii_hexdigit()))
}

impl Approvals {
	pub fn validate(&self) -> crate::Result<()> {
		if self.inference.len() > 32 || self.memory.len() > 32 {
			return Err(crate::Error::Invalid(
				"too many remote inference approvals".into(),
			));
		}
		for allowance in self.embedding.iter().chain(self.compaction.iter()) {
			if !(1..=1_000_000).contains(&allowance.call_budget)
				|| !(1..=allowance.call_budget).contains(&allowance.calls_per_agent)
			{
				return Err(crate::Error::Invalid(
					"invalid remote provider call allowance".into(),
				));
			}
		}
		for provider in self
			.inference
			.iter()
			.chain(self.memory.iter())
			.chain(self.embedding.iter().map(|a| &a.provider))
			.chain(self.compaction.iter().map(|a| &a.provider))
		{
			crate::configuration::validate_node_id(&provider.node_id)?;
			crate::nonempty(&provider.entry.id, "remote provider")?;
			crate::nonempty(&provider.entry.version, "remote provider version")?;
			if [&provider.digest, &provider.configuration_digest]
				.iter()
				.any(|s| !valid_digest(s))
			{
				return Err(crate::Error::Invalid(
					"remote provider approval requires exact definition and configuration digests"
						.into(),
				));
			}
		}
		Ok(())
	}
}

impl Purpose {
	pub fn name(self) -> &'static str {
		match self {
			Self::Embedding => "embedding",
			Self::Inference => "inference",
			Self::Compaction => "compaction",
			Self::Memory => "memory",
		}
	}
	pub fn action(self) -> &'static str {
		match self {
			Self::Embedding => "embedding.invoke",
			Self::Inference => "model.infer",
			Self::Compaction => "compaction.invoke",
			Self::Memory => "memory.invoke",
		}
	}
}

impl Usage {
	pub fn digest(&self) -> Result<String> {
		Ok(digest(&serde_json::to_value(self)?))
	}
	pub fn validate(&self) -> Result<()> {
		if [
			self.operation_id,
			self.attempt_id,
			self.grant_id,
			self.admission_id,
		]
		.iter()
		.any(Uuid::is_nil)
			|| !(1..=1_000_000_000_000_i64).contains(&self.reserved_tokens)
			|| !valid_digest(&self.input_digest)
			|| self.dispatcher_node != self.provider.node_id
		{
			return Err(ContractError::Semantic(Failure::ProviderContract));
		}
		crate::configuration::validate_node_id(&self.dispatcher_node).map_err(Into::into)
	}
}

pub fn verify_receipts(expected: &[Ancestor], receipts: &[Reserved], usage: &Usage) -> Result<()> {
	let digest = usage.digest()?;
	if expected.len() != receipts.len()
		|| expected
			.iter()
			.zip(receipts)
			.any(|(a, r)| a != &r.owner || r.attempt_id != usage.attempt_id || r.digest != digest)
	{
		return Err(ContractError::Semantic(Failure::ProviderContract));
	}
	Ok(())
}

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Finalization {
	Settled { reported: Option<i64> },
	Aborted {},
}

/// One immutable accounting decision; the reservation comes from validated Usage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settlement {
	pub state: &'static str,
	pub reported: Option<i64>,
	pub refund: i64,
	pub release_call: bool,
	pub provider_contract_violated: bool,
}

impl Finalization {
	pub fn accounting(&self, reserved_tokens: i64) -> Settlement {
		match self {
			Self::Aborted {} => Settlement {
				state: "RELEASED",
				reported: None,
				refund: reserved_tokens,
				release_call: true,
				provider_contract_violated: false,
			},
			Self::Settled { reported } => Settlement {
				state: "SETTLED",
				reported: *reported,
				refund: reported
					.filter(|tokens| *tokens > 0 && *tokens <= reserved_tokens)
					.map_or(0, |tokens| reserved_tokens - tokens),
				release_call: false,
				provider_contract_violated: reported.is_some_and(|tokens| tokens > reserved_tokens),
			},
		}
	}
}

#[derive(Debug, Clone)]
pub struct Attempt {
	pub digest: String,
	pub result: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct ReservedCharge {
	pub request_id: Uuid,
	pub state: String,
	pub reported_tokens: Option<i64>,
}

/// Persisted reservation identity is separate from a public owner receipt.
#[derive(Debug, Clone)]
pub struct ReservationBinding {
	pub digest: String,
	pub state: String,
}

pub fn ancestor(node: &str, job: &crate::generation::requests::Request) -> Ancestor {
	Ancestor {
		node_id: node.into(),
		tenant: job.tenant.clone(),
		request_id: job.id,
		policy_id: job.policy_id.clone(),
		policy_revision: job.policy_revision,
		depth: job.depth,
		expires_at: job.expires_at,
	}
}

/// Prepared remote generation may advertise lineage before activation.
pub fn lineage_live(
	job: &crate::generation::requests::Request,
	enabled: bool,
	now: chrono::DateTime<Utc>,
) -> bool {
	(job.status == "ACTIVE"
		|| (job.status == "QUEUED" && job.prepared && !job.home_node.is_empty()))
		&& job.expires_at > now
		&& enabled
		&& !job.quota_released
}
/// Provider debits require active generation even if queued lineage is prepared.
pub fn usage_live(
	job: &crate::generation::requests::Request,
	enabled: bool,
	now: chrono::DateTime<Utc>,
) -> bool {
	job.status == "ACTIVE" && job.expires_at > now && enabled && !job.quota_released
}
