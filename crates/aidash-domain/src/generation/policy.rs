//! Generation permissions, provider allowances and bounded policy specifications.
use serde::{Deserialize, Serialize};
// Serializable contracts for generation.

use crate::registry::{EntityRef, Entry};
use schemars::JsonSchema;
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "GenerationPermissions")]
#[serde(deny_unknown_fields)]
pub struct Permissions {
	#[serde(default)]
	pub roles: BTreeSet<String>,
	#[serde(default)]
	pub groups: BTreeSet<String>,
	#[serde(default = "crate::empty_object")]
	pub attributes: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "GenerationLimits")]
#[serde(deny_unknown_fields)]
pub struct Limits {
	pub max_agents: i64,
	pub max_concurrent: i64,
	pub max_depth: i32,
	pub token_budget: i64,
	pub tokens_per_agent: i64,
	pub lifetime_seconds: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "GenerationCompaction")]
#[serde(deny_unknown_fields)]
pub struct Compaction {
	pub provider: EntityRef,
	pub calls_per_agent: i64,
	pub call_budget: i64,
}

/// Separately approved Summary Stage model for generated Agents. Every
/// ancestor must approve the same exact model version.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "GenerationSummary")]
#[serde(deny_unknown_fields)]
pub struct Summary {
	pub provider: EntityRef,
	pub calls_per_agent: i64,
	pub call_budget: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "GenerationEmbedding")]
#[serde(deny_unknown_fields)]
pub struct Embedding {
	pub provider: EntityRef,
	pub calls_per_agent: i64,
	pub call_budget: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "GenerationSpec")]
pub struct Spec {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub remote: Option<crate::generation::remote::Approvals>,
	pub enabled: bool,
	pub template: Entry,
	pub permissions: Permissions,
	pub limits: Limits,
	pub approval_required: bool,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub compaction: Option<Compaction>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub embedding: Option<Embedding>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub summary: Option<Summary>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[schemars(rename = "GenerationPolicy")]
pub struct Policy {
	pub tenant: String,
	pub id: String,
	pub revision: i64,
	pub spec: Spec,
	pub generated_count: i64,
	pub allocated_tokens: i64,
	pub allocated_compaction_calls: i64,
	pub allocated_embedding_calls: i64,
	pub allocated_summary_calls: i64,
}

impl Spec {
	pub fn embedding_limits(&self) -> Option<(i64, i64)> {
		self.embedding
			.as_ref()
			.map(|c| (c.calls_per_agent, c.call_budget))
			.or_else(|| {
				self.remote
					.as_ref()?
					.embedding
					.as_ref()
					.map(|c| (c.calls_per_agent, c.call_budget))
			})
	}
}

impl Spec {
	pub fn compaction_limits(&self) -> Option<(i64, i64)> {
		self.compaction
			.as_ref()
			.map(|c| (c.calls_per_agent, c.call_budget))
			.or_else(|| {
				self.remote
					.as_ref()?
					.compaction
					.as_ref()
					.map(|c| (c.calls_per_agent, c.call_budget))
			})
	}
}

impl Spec {
	pub fn summary_limits(&self) -> Option<(i64, i64)> {
		self.summary
			.as_ref()
			.map(|c| (c.calls_per_agent, c.call_budget))
			.or_else(|| {
				self.remote
					.as_ref()?
					.summary
					.as_ref()
					.map(|c| (c.calls_per_agent, c.call_budget))
			})
	}
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Allowances {
	pub compaction_calls: i64,
	pub embedding_calls: i64,
	pub summary_calls: i64,
}
impl Policy {
	pub fn reservation(
		&self,
		active: i64,
		depth: i32,
		subject_count: usize,
	) -> crate::Result<Allowances> {
		let limits = &self.spec.limits;
		let compaction_calls = self.spec.compaction_limits().map_or(0, |(calls, _)| calls);
		let embedding_calls = self.spec.embedding_limits().map_or(0, |(calls, _)| calls);
		let summary_calls = self.spec.summary_limits().map_or(0, |(calls, _)| calls);
		if self
			.spec
			.compaction_limits()
			.is_some_and(|(calls, budget)| {
				self.allocated_compaction_calls
					.checked_add(calls)
					.is_none_or(|n| n > budget)
			}) || self.spec.embedding_limits().is_some_and(|(calls, budget)| {
			self.allocated_embedding_calls
				.checked_add(calls)
				.is_none_or(|n| n > budget)
		}) || self.spec.summary_limits().is_some_and(|(calls, budget)| {
			self.allocated_summary_calls
				.checked_add(calls)
				.is_none_or(|n| n > budget)
		}) || self.generated_count >= limits.max_agents
			|| active >= limits.max_concurrent
			|| depth > limits.max_depth
			|| subject_count >= 32
			|| self
				.allocated_tokens
				.checked_add(limits.tokens_per_agent)
				.is_none_or(|n| n > limits.token_budget)
		{
			return Err(crate::Error::Conflict(
			"generation count, concurrency, depth, token, compaction, embedding or summary budget exceeded"
				.into(),
		));
		}

		Ok(Allowances {
			compaction_calls,
			embedding_calls,
			summary_calls,
		})
	}
}
