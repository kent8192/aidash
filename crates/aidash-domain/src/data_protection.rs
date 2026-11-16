//! What Aidash asks of OpenRouter for a Tenant's content, and the Tenant
//! switches for the destinations outside the level (ADR 0137).
use crate::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The Data Protection Level. It governs only what Aidash asks of OpenRouter.
#[derive(
	Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Level {
	/// Asks OpenRouter for nothing.
	NoRestriction,
	/// Only endpoints that never train on the content.
	NoTraining,
	/// Only endpoints that keep content at most to prevent abuse.
	SafetyRetention,
	/// Only endpoints that do not keep content; Safety Retention never meets it.
	ZeroDataRetention,
}
impl Level {
	/// The Deployment floor when its settings name none.
	pub const LOWEST: Self = Self::NoRestriction;
	/// The level of a Tenant that has not chosen one.
	pub const NEW_TENANT: Self = Self::NoTraining;
}

/// The Tenant policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct DataProtection {
	pub level: Level,
	/// No Run carrying this Tenant's content uses a Plan Connection.
	pub forbid_plan_connections: bool,
	/// No Run carrying this Tenant's content calls a Tenant Keyless endpoint.
	pub forbid_keyless_endpoints: bool,
}
impl Default for DataProtection {
	fn default() -> Self {
		Self {
			level: Level::NEW_TENANT,
			forbid_plan_connections: false,
			forbid_keyless_endpoints: false,
		}
	}
}

/// The requirement signed into a model call's Capability Token.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Requirement {
	/// Strict zero data retention: Safety Retention endpoints are excluded.
	#[default]
	Zdr,
	SafetyRetention,
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
			Self::Zdr | Self::SafetyRetention => serde_json::json!({"zdr": true}),
			Self::NoTraining => serde_json::json!({"data_collection": "deny"}),
			Self::None => serde_json::json!({}),
		}
	}
	/// Whether a call must be pinned to routes read from OpenRouter's ZDR catalog.
	pub fn checks_routes(self) -> bool {
		matches!(self, Self::Zdr | Self::SafetyRetention)
	}
	/// Whether a catalog entry marked `safety_retention` meets the requirement.
	pub fn admits_safety_retention(self) -> bool {
		self != Self::Zdr
	}
}
impl From<Level> for Requirement {
	fn from(level: Level) -> Self {
		match level {
			Level::NoRestriction => Self::None,
			Level::NoTraining => Self::NoTraining,
			Level::SafetyRetention => Self::SafetyRetention,
			Level::ZeroDataRetention => Self::Zdr,
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
	OpenRouter,
	TenantKeyless,
	PlanConnection,
	ProviderTool,
	DeploymentHosted,
	/// Tenant HTTP/MCP tools, remote MCP servers and Page reading are outside this policy.
	TenantTool,
}

/// Why a destination is refused for a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
	#[error("a Tenant in this delegation chain forbids ChatGPT subscriptions")]
	PlanConnectionsForbidden,
	#[error("a Tenant in this delegation chain forbids Keyless endpoints")]
	KeylessEndpointsForbidden,
	#[error(
		"Provider tools need Data Protection Level No Restriction, but this call is held to {0:?}"
	)]
	ProviderToolsNeedNoRestriction(Level),
}

impl DataProtection {
	pub fn validate(self) -> Result<()> {
		Ok(())
	}
	pub fn with_floor(mut self, floor: Level) -> Self {
		self.level = self.level.max(floor);
		self
	}
	/// The policy of a call made for no Tenant: `zdr: true`, which is what
	/// every OpenRouter call sent before Tenants could choose a level.
	pub const TENANTLESS: Self = Self {
		level: Level::SafetyRetention,
		forbid_plan_connections: false,
		forbid_keyless_endpoints: false,
	};
	/// The strictest policy a Tenant on another Deployment might hold (#566).
	pub fn strictest() -> Self {
		Self {
			level: Level::ZeroDataRetention,
			forbid_plan_connections: true,
			forbid_keyless_endpoints: true,
		}
	}
	/// An empty chain is the default Tenant. No Tenant can weaken another one's promise.
	pub fn combine(policies: impl IntoIterator<Item = Self>, floor: Level) -> Self {
		let mut policies = policies.into_iter();
		let Some(mut combined) = policies.next() else {
			return Self::default().with_floor(floor);
		};
		for policy in policies {
			combined.level = combined.level.max(policy.level);
			combined.forbid_plan_connections |= policy.forbid_plan_connections;
			combined.forbid_keyless_endpoints |= policy.forbid_keyless_endpoints;
		}
		combined.with_floor(floor)
	}
	pub fn requirement(self) -> Requirement {
		self.level.into()
	}
	/// Why the policy refuses `destination`, if it does.
	pub fn refusal(self, destination: Destination) -> Option<Refusal> {
		match destination {
			Destination::PlanConnection if self.forbid_plan_connections => {
				Some(Refusal::PlanConnectionsForbidden)
			}
			Destination::TenantKeyless if self.forbid_keyless_endpoints => {
				Some(Refusal::KeylessEndpointsForbidden)
			}
			Destination::ProviderTool if self.level != Level::NoRestriction => {
				Some(Refusal::ProviderToolsNeedNoRestriction(self.level))
			}
			_ => None,
		}
	}
	/// Refuses, rather than falls back from, a destination the policy does not allow.
	pub fn permits(self, destination: Destination) -> Result<Requirement> {
		match self.refusal(destination) {
			Some(refusal) => Err(refusal.into()),
			None => Ok(self.requirement()),
		}
	}
}
impl From<Refusal> for crate::Error {
	fn from(refusal: Refusal) -> Self {
		Self::Invalid(format!("Data Protection: {refusal}"))
	}
}

/// Why OpenRouter's ZDR catalog offers no route for a model under a requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CatalogRefusal {
	/// The model has no zero-data-retention or Safety Retention route at all.
	NoRoute,
	/// Every route of the model is Safety Retention, which never meets strict
	/// zero data retention (#961).
	SafetyRetentionOnly,
}
impl CatalogRefusal {
	pub fn sentence(self, model: &str) -> String {
		match self {
			Self::NoRoute => format!("{model} has no current zero data retention route"),
			Self::SafetyRetentionOnly => format!(
				"{model} has only Safety Retention routes, which do not meet Zero Data Retention"
			),
		}
	}
}

/// Why one pinned route fails the ZDR catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinnedRefusal {
	NotListed,
	SafetyRetentionOnly,
}

/// A missing or non-boolean `safety_retention` counts as Safety Retention.
pub fn is_safety_retention(entry: &serde_json::Value) -> bool {
	entry["safety_retention"].as_bool() != Some(false)
}

/// The tags `provider.only` may list for `model` under `requirement`, read from
/// the `data` array of OpenRouter's `/endpoints/zdr`.
///
/// Strict zero data retention matches Safety Retention per `(model_id, tag)`,
/// since one tag can be Safety Retention for one model and strict for
/// another. It also drops a parent of a Safety Retention tag, because
/// `provider.only` admits every tag below the ones it lists.
pub fn admitted_routes(
	catalog: &[serde_json::Value],
	model: &str,
	requirement: Requirement,
) -> std::result::Result<Vec<String>, CatalogRefusal> {
	let entries: Vec<&serde_json::Value> = catalog
		.iter()
		.filter(|entry| entry["model_id"] == model)
		.collect();
	let retained: Vec<&str> = entries
		.iter()
		.filter(|entry| is_safety_retention(entry))
		.filter_map(|entry| entry["tag"].as_str())
		.collect();
	let mut routes: Vec<String> = entries
		.iter()
		.filter_map(|entry| entry["tag"].as_str())
		.filter(|tag| {
			requirement.admits_safety_retention()
				|| !retained.iter().any(|retained| {
					retained == tag
						|| retained
							.strip_prefix(tag)
							.is_some_and(|rest| rest.starts_with('/'))
				})
		})
		.map(str::to_owned)
		.collect();
	routes.sort();
	routes.dedup();
	if routes.is_empty() {
		return Err(if retained.is_empty() {
			CatalogRefusal::NoRoute
		} else {
			CatalogRefusal::SafetyRetentionOnly
		});
	}
	Ok(routes)
}

/// Why a pinned route does not meet `requirement`, judged on the ZDR catalog.
pub fn pinned_route_refusal(
	catalog: &[serde_json::Value],
	model: &str,
	tag: &str,
	requirement: Requirement,
) -> Option<PinnedRefusal> {
	let entries: Vec<&serde_json::Value> = catalog
		.iter()
		.filter(|entry| entry["model_id"] == model && entry["tag"] == tag)
		.collect();
	if entries.is_empty() {
		Some(PinnedRefusal::NotListed)
	} else if !requirement.admits_safety_retention()
		&& entries.iter().any(|entry| is_safety_retention(entry))
	{
		Some(PinnedRefusal::SafetyRetentionOnly)
	} else {
		None
	}
}

/// Whether a request body's `provider.only` keeps a strict zero data
/// retention call off Safety Retention: it must list at least one tag, and
/// every listed tag must be admitted for `model`.
pub fn only_meets(
	only: Option<&serde_json::Value>,
	catalog: &[serde_json::Value],
	model: &str,
	requirement: Requirement,
) -> bool {
	let Some(tags) = only.and_then(serde_json::Value::as_array) else {
		return false;
	};
	let Ok(admitted) = admitted_routes(catalog, model, requirement) else {
		return false;
	};
	!tags.is_empty()
		&& tags.iter().all(|tag| {
			tag.as_str()
				.is_some_and(|tag| admitted.iter().any(|admitted| admitted == tag))
		})
}

#[cfg(test)]
mod tests;
