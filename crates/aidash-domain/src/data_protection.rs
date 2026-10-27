//! What Aidash requests of destinations and what their published terms promise.
use crate::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

mod region;
pub use region::{Region, RegionRestriction};

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
	/// Restricts content to this Tenant's Deployment Region; never names a host.
	#[serde(skip_serializing_if = "std::ops::Not::not")]
	pub data_region_restricted: bool,
	/// Bound by the Deployment, then intersected across the delegation chain.
	#[serde(skip)]
	pub region_restriction: RegionRestriction,
	pub level: Level,
	pub model_call_zdr: bool,
	pub plan_connection_exempt: bool,
}
impl Default for DataProtection {
	fn default() -> Self {
		Self {
			data_region_restricted: false,
			region_restriction: RegionRestriction::Any,
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
	DeploymentHosted(Option<Region>),
	/// Tenant HTTP/MCP tools and remote MCP servers are outside this policy.
	TenantTool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permission {
	pub requirement: Requirement,
	pub plan_connection_exemption: bool,
}
impl DataProtection {
	pub fn in_deployment(mut self, region: Option<Region>) -> Self {
		self.region_restriction = if self.data_region_restricted {
			region.map_or(RegionRestriction::Unsatisfiable, RegionRestriction::Within)
		} else {
			RegionRestriction::Any
		};
		self
	}
	pub fn region(self) -> Result<Option<Region>> {
		if self.data_region_restricted && self.region_restriction == RegionRestriction::Any {
			return RegionRestriction::Unsatisfiable.region();
		}
		self.region_restriction.region()
	}
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
		if combined.data_region_restricted && combined.region_restriction == RegionRestriction::Any
		{
			combined.region_restriction = RegionRestriction::Unsatisfiable;
		}
		for mut policy in policies {
			if policy.data_region_restricted && policy.region_restriction == RegionRestriction::Any
			{
				policy.region_restriction = RegionRestriction::Unsatisfiable;
			}
			combined.region_restriction = combined
				.region_restriction
				.intersect(policy.region_restriction);
			combined.data_region_restricted |= policy.data_region_restricted;
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
		let region = self.region()?;
		if let Some(region) = region {
			let reason = match destination {
				Destination::OpenRouter => {
					region.openrouter_endpoint()?;
					None
				}
				Destination::DeploymentHosted(hosted) if hosted != Some(region) => {
					Some("Deployment-hosted model is outside the required region")
				}
				Destination::TenantKeyless => {
					Some("Tenant Keyless endpoints have no verified region")
				}
				Destination::PlanConnection if !self.plan_connection_exempt => Some(
					"Plan Connections require the Tenant exemption under the Data Region restriction",
				),
				Destination::Research(terms) if terms.data_region != Some(region) => {
					Some("Research account terms do not record the required region")
				}
				_ => None,
			};
			if let Some(reason) = reason {
				return Err(Error::Invalid(format!(
					"Data Region restriction {region:?}: {reason}"
				)));
			}
		}
		let refused = |reason: &str| {
			Error::Invalid(format!("Data Protection Level {:?}: {reason}", self.level))
		};
		let exemption = match destination {
			Destination::OpenRouter
			| Destination::DeploymentHosted(_)
			| Destination::TenantTool => false,
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
