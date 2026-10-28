//! Deployment jurisdictions and the intersection of residency promises.
use crate::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(
	Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Region {
	Eu,
	Us,
	Jp,
}
impl Region {
	pub const ALL: [Self; 3] = [Self::Eu, Self::Us, Self::Jp];
	/// Fixed catalog endpoints. A Tenant cannot supply a regional hostname.
	pub fn openrouter_endpoint(self) -> Result<&'static str> {
		match self {
			Self::Eu => Ok("https://eu.openrouter.ai/api/v1"),
			Self::Us => Ok("https://us.openrouter.ai/api/v1"),
			Self::Jp => Err(Error::Invalid(
				"Data Region restriction: OpenRouter has no JP endpoint".into(),
			)),
		}
	}
	pub fn openrouter_data_region(self) -> &'static str {
		match self {
			Self::Eu => "eu",
			Self::Us => "us",
			Self::Jp => "jp",
		}
	}
	/// Conservative engine check in addition to the current regional tools catalog.
	/// The native engine's inference residency promise does not cover web search.
	pub fn permits_openrouter_tool(self, tool: &str, engine: Option<&str>) -> bool {
		match tool {
			"openrouter:datetime"
			| "openrouter:apply_patch"
			| "openrouter:advisor"
			| "openrouter:subagent"
			| "openrouter:tool_search"
			| "openrouter:experimental__search_models" => self != Self::Jp,
			"openrouter:web_search" => {
				self == Self::Us && matches!(engine, None | Some("auto" | "exa"))
			}
			// Sandbox containers (shell, bash) are served by the global endpoint
			// only; OpenRouter rejects them on the in-region hosts.
			"openrouter:shell" | "openrouter:bash" => false,
			_ => false,
		}
	}
}

/// Evaluated state, never accepted from a Tenant's request body.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RegionRestriction {
	#[default]
	Any,
	Within(Region),
	/// Includes an enabled restriction without a Deployment declaration.
	Unsatisfiable,
}
impl RegionRestriction {
	pub fn intersect(self, other: Self) -> Self {
		match (self, other) {
			(Self::Any, other) | (other, Self::Any) => other,
			(Self::Within(a), Self::Within(b)) if a == b => Self::Within(a),
			_ => Self::Unsatisfiable,
		}
	}
	pub fn region(self) -> Result<Option<Region>> {
		match self {
			Self::Any => Ok(None),
			Self::Within(region) => Ok(Some(region)),
			Self::Unsatisfiable => Err(Error::Invalid("Data Region restriction: Deployment Region is missing or delegation regions do not overlap".into())),
		}
	}
}
