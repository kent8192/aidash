use aidash_domain::semantic::EmbeddingConfig;
use serde_json::json;

#[test]
fn embedding_registration_accepts_explicit_text_and_image_inputs() {
	let config = json!({
		"provider": "openrouter", "endpoint": "https://openrouter.ai/api/v1",
		"model": "google/gemini-embedding-2", "model_version": "1.0.0",
		"dimensions": 3072, "inputs": ["text", "image"]
	});
	let parsed: EmbeddingConfig = serde_json::from_value(config.clone()).unwrap();
	parsed.validate_parameters().unwrap();
	assert_eq!(serde_json::to_value(parsed).unwrap(), config);
}

#[test]
fn omitted_inputs_are_text_only_and_text_is_required() {
	let mut value = json!({"provider":"openai","endpoint":"https://example.com/v1", "model":"fixture", "model_version":"1", "dimensions":2});
	let config: EmbeddingConfig = serde_json::from_value(value.clone()).unwrap();
	assert_eq!(
		serde_json::to_value(config).unwrap()["inputs"],
		json!(["text"])
	);
	value["inputs"] = json!(["image"]);
	let config: EmbeddingConfig = serde_json::from_value(value).unwrap();
	assert!(config.validate_parameters().is_err());
}

#[test]
fn image_reservation_uses_media_allowance_not_base64_size() {
	use aidash_domain::semantic::EmbeddingInput;
	let bytes = vec![0u8; 257];
	let image = EmbeddingInput::Image {
		media_type: "image/png",
		bytes: &bytes,
	};
	assert_eq!(image.reservation_amount().unwrap(), 4096 + 2 + 9 + 1024);
	assert_eq!(
		EmbeddingInput::Text("abc").reservation_amount().unwrap(),
		1027
	);
}
