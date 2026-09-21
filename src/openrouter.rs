//! Public model catalog used by the model registration picker.
use crate::Result;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
pub struct CatalogModel {
	pub id: String,
	pub name: String,
	pub context_length: usize,
	pub pricing: Pricing,
	pub architecture: Architecture,
	#[serde(default)]
	pub supported_parameters: Vec<String>,
	pub reasoning: Option<ReasoningOptions>,
}

#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
pub struct Pricing {
	pub prompt: String,
	pub completion: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
pub struct Architecture {
	pub input_modalities: Vec<String>,
	pub output_modalities: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
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

#[derive(Deserialize)]
struct Catalog {
	data: Vec<CatalogModel>,
}

pub async fn models(client: &reqwest::Client) -> Result<Vec<CatalogModel>> {
	// This is a public endpoint; never send a node or provider credential.
	let response = client
		.get("https://openrouter.ai/api/v1/models")
		.timeout(Duration::from_secs(15))
		.send()
		.await?
		.error_for_status()?;
	let catalog: Catalog = crate::response::json(response, 8 * 1024 * 1024).await?;
	Ok(eligible(catalog.data))
}

fn eligible(models: Vec<CatalogModel>) -> Vec<CatalogModel> {
	let mut models: Vec<_> = models
		.into_iter()
		.filter(|m| {
			!m.id.is_empty()
				&& m.context_length >= 2048
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
	use serde_json::json;

	#[test]
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

	#[test]
	fn catalog_only_offers_models_usable_by_agents() {
		let model = json!({
			"id":"vendor/model", "name":"Text model", "context_length":32768,
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
	}
}
