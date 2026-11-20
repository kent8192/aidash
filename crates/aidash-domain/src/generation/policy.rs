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
	/// How many agents the policy may generate in all; absent means no cap
	/// (#1012, ADR 0143).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub max_agents: Option<i64>,
	/// How many may be active at once; absent means no cap.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub max_concurrent: Option<i64>,
	/// How deeply generated agents may generate in turn; absent means no cap.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub max_depth: Option<i32>,
	/// How many tokens the policy's generated agents may use in all; absent
	/// means no token limit. Money Budgets are the limit then (#1012
	/// decision 41, ADR 0143).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub token_budget: Option<i64>,
	/// How many tokens one generated agent may use; absent means no token
	/// limit. Required with `token_budget`, which it is reserved against.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub tokens_per_agent: Option<i64>,
	/// How long a generated agent lasts; absent means as long as its Task, with
	/// no maximum.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub lifetime_seconds: Option<i64>,
}

/// The lifetime recorded for a generated agent whose policy names none: one
/// hundred years, so it lasts as long as its Task.
pub const UNBOUNDED_LIFETIME_SECONDS: i64 = 100 * 365 * 24 * 60 * 60;

/// The token limit recorded for a generated agent whose policy names none:
/// more than any policy may set (10^15), and still exact as a JSON number.
/// A request recorded with it reserved nothing of its policy's tokens.
pub const UNBOUNDED_TOKENS: i64 = 1_000_000_000_000_000;

impl Limits {
	/// The lifetime to record for a generated agent.
	pub fn lifetime(&self) -> i64 {
		self.lifetime_seconds.unwrap_or(UNBOUNDED_LIFETIME_SECONDS)
	}

	/// The token limit to record for a generated agent.
	pub fn agent_token_limit(&self) -> i64 {
		self.tokens_per_agent.unwrap_or(UNBOUNDED_TOKENS)
	}

	/// What one generated agent reserves of the policy's tokens: nothing
	/// when the policy sets no per-agent limit.
	pub fn reserved_tokens(&self) -> i64 {
		self.tokens_per_agent.unwrap_or(0)
	}
}

/// A Generation policy's free-form mode (#1012, ADR 0143): the Agent writes the
/// instructions and picks components within what a person set here, instead of
/// copying the template unchanged. The template still supplies everything the
/// draft does not name.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "GenerationFreeForm")]
#[serde(deny_unknown_fields)]
pub struct FreeForm {
	/// Any tool, Skill or model already Catalog-approved for the Node may be
	/// picked; the Catalog Approval checks still run on every pick.
	#[serde(default)]
	pub any_approved: bool,
	/// Otherwise only these.
	#[serde(default)]
	pub components: Vec<EntityRef>,
}

impl FreeForm {
	pub fn allows(&self, reference: &EntityRef) -> bool {
		self.any_approved || self.components.contains(reference)
	}
}

/// What an Agent writes for a free-form generated agent.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "GenerationDraft")]
#[serde(deny_unknown_fields)]
pub struct Draft {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub name: Option<String>,
	pub instructions: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub model: Option<EntityRef>,
	/// Tools and Skills, each bound under its own id.
	#[serde(default)]
	pub components: Vec<EntityRef>,
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
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub free_form: Option<FreeForm>,
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
	pub fn reservation(&self, active: i64, depth: i32) -> crate::Result<Allowances> {
		let limits = &self.spec.limits;
		let embedding_calls = self.spec.embedding_limits().map_or(0, |(calls, _)| calls);
		if self.spec.embedding_limits().is_some_and(|(calls, budget)| {
			self.allocated_embedding_calls
				.checked_add(calls)
				.is_none_or(|n| n > budget)
		}) || limits
			.max_agents
			.is_some_and(|max| self.generated_count >= max)
			|| limits.max_concurrent.is_some_and(|max| active >= max)
			|| limits.max_depth.is_some_and(|max| depth > max)
			|| limits.token_budget.is_some_and(|budget| {
				self.allocated_tokens
					.checked_add(limits.reserved_tokens())
					.is_none_or(|n| n > budget)
			}) {
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

#[cfg(test)]
mod token_limit_tests {
	use super::*;
	use rstest::rstest;

	fn policy(token_budget: Option<i64>, tokens_per_agent: Option<i64>, allocated: i64) -> Policy {
		let spec: Spec = serde_json::from_value(serde_json::json!({
			"enabled":true,
			"template":{"id":"agent","version":"1.0.0","kind":"agent","name":{},"description":{},"config":{}},
			"permissions":{},
			"limits":{"token_budget":token_budget,"tokens_per_agent":tokens_per_agent},
			"approval_required":false
		}))
		.unwrap();
		Policy {
			tenant: "acme".into(),
			id: "policy".into(),
			revision: 1,
			spec,
			generated_count: 3,
			allocated_tokens: allocated,
			allocated_embedding_calls: 0,
		}
	}

	/// Without token limits a policy generates past what an old token budget
	/// allowed; Budgets limit its spend instead (#1012 decision 41).
	#[rstest]
	#[case::no_token_limits(None, None, 1_000_000_000, true)]
	#[case::per_agent_only(None, Some(1000), 1_000_000_000, true)]
	#[case::within_the_budget(Some(3000), Some(1000), 2000, true)]
	#[case::past_the_budget(Some(3000), Some(1000), 2001, false)]
	fn a_token_budget_limits_generation_only_when_set(
		#[case] token_budget: Option<i64>,
		#[case] tokens_per_agent: Option<i64>,
		#[case] allocated: i64,
		#[case] allowed: bool,
	) {
		// Act
		let reservation = policy(token_budget, tokens_per_agent, allocated).reservation(0, 1);
		// Assert
		assert_eq!(reservation.is_ok(), allowed);
	}

	#[test]
	fn an_agent_without_a_token_limit_records_the_unbounded_limit_and_reserves_nothing() {
		// Arrange
		let limits = policy(None, None, 0).spec.limits;
		// Act / Assert
		assert_eq!(limits.agent_token_limit(), UNBOUNDED_TOKENS);
		assert_eq!(limits.reserved_tokens(), 0);
		assert!(
			serde_json::to_value(&limits)
				.unwrap()
				.get("token_budget")
				.is_none()
		);
	}
}
