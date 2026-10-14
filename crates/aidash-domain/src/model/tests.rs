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
