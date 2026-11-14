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
	pub embedding: Option<Embedding>,
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
	pub allocated_embedding_calls: i64,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Allowances {
	pub embedding_calls: i64,
}
impl Policy {
	pub fn reservation(
		&self,
		active: i64,
		depth: i32,
		subject_count: usize,
	) -> crate::Result<Allowances> {
		let limits = &self.spec.limits;
		let embedding_calls = self.spec.embedding_limits().map_or(0, |(calls, _)| calls);
		if self.spec.embedding_limits().is_some_and(|(calls, budget)| {
			self.allocated_embedding_calls
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
				"generation count, concurrency, depth, token or embedding budget exceeded".into(),
			));
		}

		Ok(Allowances { embedding_calls })
	}
}

#[cfg(test)]
mod removed_compaction_tests {
	use super::*;

	#[test]
	fn compaction_is_not_an_admissible_generation_setting() {
		let mut input = serde_json::json!({
			"enabled":true,
			"template":{"id":"agent","version":"1.0.0","kind":"agent","name":{},"description":{},"config":{}},
			"permissions":{},
			"limits":{"max_agents":1,"max_concurrent":1,"max_depth":1,"token_budget":1000,"tokens_per_agent":1000,"lifetime_seconds":60},
			"approval_required":false,
			"compaction":{"provider":{"id":"judge","version":"1.0.0"},"calls_per_agent":1,"call_budget":1}
		});
		assert!(serde_json::from_value::<Spec>(input.clone()).is_err());
		input.as_object_mut().unwrap().remove("compaction");
		assert!(serde_json::from_value::<Spec>(input).is_ok());
	}

	#[test]
	fn separate_summary_model_is_not_an_admissible_generation_setting() {
		let mut input = serde_json::json!({
			"enabled":true,
			"template":{"id":"agent","version":"1.0.0","kind":"agent","name":{},"description":{},"config":{}},
			"permissions":{},
			"limits":{"max_agents":1,"max_concurrent":1,"max_depth":1,"token_budget":1000,"tokens_per_agent":1000,"lifetime_seconds":60},
			"approval_required":false,
			"summary":{"provider":{"id":"model","version":"1.0.0"},"calls_per_agent":1,"call_budget":1}
		});
		assert!(serde_json::from_value::<Spec>(input.clone()).is_err());
		input.as_object_mut().unwrap().remove("summary");
		assert!(serde_json::from_value::<Spec>(input).is_ok());
	}
}
