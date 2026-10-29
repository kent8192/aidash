//! What Aidash requests of destinations and what their published terms promise.
use crate::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(
	Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Level {
	#[default]
	Standard,
	NoTraining,
	ZeroDataRetention,
}

/// The Tenant policy. Missing settings preserve the existing OpenRouter ZDR default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct DataProtection {
	pub level: Level,
	pub model_call_zdr: bool,
	pub plan_connection_exempt: bool,
}
impl Default for DataProtection {
	fn default() -> Self {
		Self {
			level: Level::Standard,
			model_call_zdr: true,
			plan_connection_exempt: true,
		}
	}
}

/// The requirement signed into a model call's Capability Token.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Requirement {
	#[default]
	Zdr,
	NoTraining,
	None,
}
impl Requirement {
	pub fn is_default(&self) -> bool {
		*self == Self::Zdr
	}
	/// Only data policy fields; routing and parameter requirements are independent.
	pub fn provider(self) -> serde_json::Value {
		match self {
			Self::Zdr => serde_json::json!({"zdr": true}),
			Self::NoTraining => serde_json::json!({"data_collection": "deny"}),
			Self::None => serde_json::json!({}),
		}
	}
}

#[derive(Debug, Clone, Copy)]
pub enum Destination<'a> {
	OpenRouter,
	TenantKeyless,
	PlanConnection,
	ProviderTool,
	Research(&'a crate::research::account::AccountTerms),
	DeploymentHosted,
	/// Tenant HTTP/MCP tools and remote MCP servers are outside this policy.
	TenantTool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permission {
	pub requirement: Requirement,
	pub plan_connection_exemption: bool,
}
impl DataProtection {
	pub fn validate(self) -> Result<()> {
		if self.level == Level::ZeroDataRetention && !self.model_call_zdr {
			return Err(Error::Invalid(
				"Zero Data Retention requires model-call ZDR".into(),
			));
		}
		Ok(())
	}
	pub fn with_floor(mut self, floor: Level) -> Self {
		self.level = self.level.max(floor);
		self.model_call_zdr |= self.level == Level::ZeroDataRetention;
		self
	}
	/// An empty chain is the default Tenant. No Tenant can weaken another one's promise.
	pub fn combine(policies: impl IntoIterator<Item = Self>, floor: Level) -> Self {
		let mut policies = policies.into_iter();
		let Some(mut combined) = policies.next() else {
			return Self::default().with_floor(floor);
		};
		for policy in policies {
			combined.level = combined.level.max(policy.level);
			combined.model_call_zdr |= policy.model_call_zdr;
			combined.plan_connection_exempt &= policy.plan_connection_exempt;
		}
		combined.with_floor(floor)
	}
	pub fn requirement(self) -> Requirement {
		if self.model_call_zdr || self.level == Level::ZeroDataRetention {
			Requirement::Zdr
		} else if self.level == Level::NoTraining {
			Requirement::NoTraining
		} else {
			Requirement::None
		}
	}
	pub fn permits(self, destination: Destination<'_>) -> Result<Permission> {
		let refused = |reason: &str| {
			Error::Invalid(format!("Data Protection Level {:?}: {reason}", self.level))
		};
		let exemption = match destination {
			Destination::OpenRouter | Destination::DeploymentHosted | Destination::TenantTool => {
				false
			}
			Destination::PlanConnection if self.plan_connection_exempt => true,
			Destination::PlanConnection if self.level != Level::Standard => {
				return Err(refused(
					"Plan Connections require the Tenant exemption at this level",
				));
			}
			Destination::TenantKeyless if self.level != Level::Standard => {
				return Err(refused("Tenant Keyless endpoints cannot meet this level"));
			}
			Destination::ProviderTool if self.level != Level::Standard => {
				return Err(refused("Provider tools cannot meet this level"));
			}
			Destination::Research(terms)
				if self.level == Level::ZeroDataRetention
					&& !terms.request_retention_prohibited =>
			{
				return Err(refused(
					"Research account terms do not prohibit request retention",
				));
			}
			Destination::Research(terms)
				if self.level >= Level::NoTraining && !terms.query_training_prohibited =>
			{
				return Err(refused("Research account terms do not prohibit training"));
			}
			_ => false,
		};
		Ok(Permission {
			requirement: self.requirement(),
			plan_connection_exemption: exemption,
		})
	}
}

#[cfg(test)]
mod tests;
