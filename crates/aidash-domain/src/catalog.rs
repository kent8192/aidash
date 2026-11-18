//! Model capabilities used to select definitions for Agents.
use serde::{Deserialize, Serialize};

/// The eligible OpenRouter models as the server last fetched them (#362).
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct CatalogSnapshot {
	pub models: Vec<CatalogModel>,
	/// The last refresh failed, so these are the models of an earlier one.
	pub stale: bool,
	pub fetched_at: chrono::DateTime<chrono::Utc>,
	/// What OpenRouter's ZDR list offered every model it named, embedding
	/// models included; empty when the list could not be read (#963).
	pub zero_data_retention: std::collections::BTreeMap<String, ZdrRoutes>,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct CatalogModel {
	pub id: String,
	pub name: String,
	pub context_length: usize,
	#[serde(default)]
	pub top_provider: Option<TopProvider>,
	pub pricing: Pricing,
	pub architecture: Architecture,
	#[serde(default)]
	pub supported_parameters: Vec<String>,
	pub reasoning: Option<ReasoningOptions>,
	/// What OpenRouter's ZDR catalog offers this model; absent when the
	/// catalog could not be read. The picker uses it to say which Data
	/// Protection Levels the model can meet (#963).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub zero_data_retention: Option<ZdrRoutes>,
}

/// The routes OpenRouter's ZDR catalog lists for one model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ZdrRoutes {
	/// At least one route meets strict zero data retention.
	Strict,
	/// Every route keeps content to prevent abuse: usable up to Safety
	/// Retention, never at Zero Data Retention.
	SafetyRetentionOnly,
	/// No route at all: usable only at No Restriction and No Training.
	None,
}
impl ZdrRoutes {
	pub fn of(catalog: &[serde_json::Value], model: &str) -> Self {
		use crate::data_protection::{Requirement, admitted_routes};
		if admitted_routes(catalog, model, Requirement::Zdr).is_ok() {
			Self::Strict
		} else if admitted_routes(catalog, model, Requirement::SafetyRetention).is_ok() {
			Self::SafetyRetentionOnly
		} else {
			Self::None
		}
	}
	/// Whether a call held to `level` can use the model.
	pub fn meets(self, level: crate::data_protection::Level) -> bool {
		use crate::data_protection::Level;
		match level {
			Level::NoRestriction | Level::NoTraining => true,
			Level::SafetyRetention => self != Self::None,
			Level::ZeroDataRetention => self == Self::Strict,
		}
	}
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct TopProvider {
	pub max_completion_tokens: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct Pricing {
	pub prompt: String,
	pub completion: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct Architecture {
	pub input_modalities: Vec<String>,
	pub output_modalities: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ReasoningOptions {
	// Absent means no effort selector; explicit null means all gateway levels.
	#[serde(default, deserialize_with = "supported_efforts")]
	pub supported_efforts: Option<Vec<String>>,
	pub default_effort: Option<String>,
	#[serde(default)]
	pub mandatory: bool,
}

fn supported_efforts<'de, D: serde::Deserializer<'de>>(
	deserializer: D,
) -> std::result::Result<Option<Vec<String>>, D::Error> {
	Ok(Some(
		Option::<Vec<String>>::deserialize(deserializer)?.unwrap_or_else(|| {
			["max", "xhigh", "high", "medium", "low", "minimal", "none"]
				.into_iter()
				.map(str::to_owned)
				.collect()
		}),
	))
}

pub fn eligible(models: Vec<CatalogModel>) -> Vec<CatalogModel> {
	let mut models: Vec<_> = models
		.into_iter()
		.filter(|m| {
			!m.id.is_empty()
				&& m.context_length >= 2048
				&& m.top_provider
					.as_ref()
					.and_then(|p| p.max_completion_tokens)
					.is_some_and(|tokens| tokens > 0 && tokens as usize <= m.context_length)
				&& m.architecture.input_modalities.iter().any(|v| v == "text")
				&& m.architecture.output_modalities.iter().any(|v| v == "text")
				&& m.supported_parameters.iter().any(|v| v == "tools")
		})
		.collect();
	models.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
	models
}

#[cfg(test)]
mod tests {
	use super::*;
	#[derive(Deserialize)]
	struct Catalog {
		data: Vec<CatalogModel>,
	}
	use serde_json::json;

	#[rstest::rstest]
	fn reasoning_distinguishes_missing_and_unrestricted_efforts() {
		let absent: ReasoningOptions = serde_json::from_value(json!({"mandatory":true})).unwrap();
		assert!(absent.supported_efforts.is_none());
		let unrestricted: ReasoningOptions =
			serde_json::from_value(json!({"supported_efforts":null})).unwrap();
		assert_eq!(unrestricted.supported_efforts.unwrap().len(), 7);
		let restricted: ReasoningOptions =
			serde_json::from_value(json!({"supported_efforts":["high","low"],"mandatory":true}))
				.unwrap();
		assert_eq!(restricted.supported_efforts.unwrap(), vec!["high", "low"]);
		assert!(restricted.mandatory);
	}

	#[rstest::rstest]
	fn catalog_only_offers_models_usable_by_agents() {
		let model = json!({
			"id":"vendor/model", "name":"Text model", "context_length":32768,
			"top_provider":{"max_completion_tokens":8192},
			"pricing":{"prompt":"0.000001","completion":"0.000002"},
			"architecture":{"input_modalities":["text","image"],"output_modalities":["text"]},
			"supported_parameters":["tools"]
		});
		let mut without_tools = model.clone();
		without_tools["supported_parameters"] = json!([]);
		let mut image_only = model.clone();
		image_only["architecture"]["output_modalities"] = json!(["image"]);
		let mut short_context = model.clone();
		short_context["context_length"] = json!(1024);
		let catalog: Catalog = serde_json::from_value(
			json!({"data":[without_tools, model, image_only, short_context]}),
		)
		.unwrap();
		let result = eligible(catalog.data);
		assert_eq!(result.len(), 1);
		assert_eq!(result[0].id, "vendor/model");
		assert_eq!(result[0].pricing.prompt, "0.000001");
		assert_eq!(
			result[0]
				.top_provider
				.as_ref()
				.unwrap()
				.max_completion_tokens,
			Some(8192)
		);
	}

	#[rstest::rstest]
	fn catalog_rejects_models_without_a_known_output_limit() {
		let mut unknown_limit = json!({
			"id":"vendor/model", "name":"Text model", "context_length":32768,
			"pricing":{"prompt":"0.000001","completion":"0.000002"},
			"architecture":{"input_modalities":["text"],"output_modalities":["text"]},
			"supported_parameters":["tools"]
		});
		unknown_limit["top_provider"] = json!({"max_completion_tokens":null});
		let catalog: Catalog = serde_json::from_value(json!({"data":[unknown_limit]})).unwrap();
		assert!(eligible(catalog.data).is_empty());
	}

	#[rstest::rstest]
	fn catalog_rejects_output_limits_larger_than_the_context_window() {
		let catalog: Catalog = serde_json::from_value(json!({"data":[{
			"id":"vendor/model", "name":"Text model", "context_length":32768,
			"top_provider":{"max_completion_tokens":65536},
			"pricing":{"prompt":"0.000001","completion":"0.000002"},
			"architecture":{"input_modalities":["text"],"output_modalities":["text"]},
			"supported_parameters":["tools"]
		}]}))
		.unwrap();
		assert!(eligible(catalog.data).is_empty());
	}
}
