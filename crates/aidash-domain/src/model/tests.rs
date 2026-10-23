use super::*;
use serde_json::json;

fn plan_model(modalities: &[&str]) -> ModelConfig {
	serde_json::from_value(json!({
		"provider": "chatgpt",
		"model_id": "gpt-fixture",
		"endpoint": "https://api.openai.com/v1",
		"plan_connection": "chatgpt",
		"context_window": 128000,
		"max_output_tokens": 16384,
		"modalities": modalities,
		"cost": {},
	}))
	.unwrap()
}

/// A plan model has no OpenRouter routes; its declared image modality is
/// still advertised so clients can offer image uploads, and audio never is.
#[test]
fn plan_models_advertise_their_declared_image_route() {
	let routes = plan_model(&["text", "image"]).current_media_input_routes();
	assert_eq!(routes.len(), 1);
	assert!(
		routes[0]
			.iter()
			.all(|media_type| media_type.starts_with("image/"))
	);
	assert!(routes[0].contains(&"image/png".to_owned()));
	assert!(
		plan_model(&["text"])
			.current_media_input_routes()
			.is_empty()
	);
}

/// Native is a supported plan projection without relaxing the plan's stream,
/// media or cache restrictions.
#[test]
fn native_plan_models_keep_the_plan_usage_restrictions() {
	let mut model = plan_model(&["text", "image"]);
	model.projection_versions = vec![crate::projection::ProjectionVersion::Native];
	model.validate_plan_usage().unwrap();
	model
		.require_projection(crate::projection::ProjectionVersion::Native)
		.unwrap();
	for (field, value) in [
		("streaming", json!(false)),
		("cache_mode", json!("explicit")),
		("modalities", json!(["text", "audio"])),
		(
			"media_routes",
			json!([{
				"tag":"fixture", "formats":["image/png"], "source":"fixture",
				"verified_at":"2026-10-23T00:00:00Z", "expires_at":"2026-10-24T00:00:00Z"
			}]),
		),
	] {
		let mut value_model = serde_json::to_value(&model).unwrap();
		value_model[field] = value;
		let invalid: ModelConfig = serde_json::from_value(value_model).unwrap();
		assert!(invalid.validate_plan_usage().is_err(), "{field}");
	}
}
