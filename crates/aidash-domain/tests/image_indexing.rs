use aidash_domain::semantic::indexing::IndexingSpec;
use serde_json::json;

#[test]
fn index_configuration_accepts_image_byte_and_count_limits() {
	let spec = json!({
		"embedding": {"provider":"openrouter", "endpoint":"https://openrouter.ai/api/v1", "model":"google/gemini-embedding-2", "model_version":"1", "dimensions":3072, "inputs":["text", "image"]},
		"vector": {"provider":"postgres", "endpoint":"local", "credential_env":null},
		"enabled":true, "auto_context":true, "max_sources":256, "max_results":10,
		"max_result_tokens":4096, "max_input_bytes":8192,
		"max_image_bytes":1048576, "max_images":24
	});
	let parsed: IndexingSpec = serde_json::from_value(spec.clone()).unwrap();
	assert_eq!(serde_json::to_value(parsed).unwrap(), spec);
}

#[test]
fn working_area_image_source_keeps_only_a_version_pointer() {
	let pointer = json!({"kind":"working_image", "area_id":"00000000-0000-0000-0000-000000000001", "file_id":"00000000-0000-0000-0000-000000000002", "path":"pictures/photo.png"});
	let source: aidash_domain::semantic::Source = serde_json::from_value(pointer.clone()).unwrap();
	assert_eq!(serde_json::to_value(source).unwrap(), pointer);
}
