use aidash::{
	provider::{ModelRequest, ToolSpec, provider},
	registry::{Entry, ModelConfig, validate},
};
use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use std::time::Duration;

fn config(provider: &str, endpoint: String) -> ModelConfig {
	ModelConfig {
		provider: provider.into(),
		model_id: "vendor/fixture-model".into(),
		endpoint,
		credential_env: None,
		reasoning_effort: None,
		context_window: 128000,
		max_output_tokens: Some(65536),
		modalities: vec!["text".into()],
		cost: json!({}),
	}
}

#[test]
fn model_configuration_uses_the_catalog_limit_and_preserves_legacy_records() {
	let selected: ModelConfig = serde_json::from_value(json!({
		"provider":"openrouter", "model_id":"google/gemini-3.8-flash",
		"endpoint":"https://openrouter.ai/api/v1", "credential_env":null,
		"context_window":1048576, "max_output_tokens":65536,
		"modalities":["text"], "cost":{}
	}))
	.unwrap();
	assert_eq!(selected.output_token_limit(), 65536);

	let legacy: ModelConfig = serde_json::from_value(json!({
		"provider":"openrouter", "model_id":"vendor/legacy",
		"endpoint":"https://openrouter.ai/api/v1", "credential_env":null,
		"context_window":128000, "modalities":["text"], "cost":{}
	}))
	.unwrap();
	assert_eq!(legacy.output_token_limit(), 4096);
}

#[test]
fn local_model_registration_requires_a_valid_catalog_output_limit() {
	let mut entry: Entry = serde_json::from_value(json!({
		"id":"router-model","version":"1.0.0","kind":"model",
		"name":{"en":"Router model"},"description":{"en":"Test model"},
		"config":config("openrouter","https://openrouter.ai/api/v1".into())
	}))
	.unwrap();
	validate(&entry).unwrap();
	entry.config["max_output_tokens"] = json!(0);
	assert!(validate(&entry).is_err());
	entry.config["max_output_tokens"] = json!(131072);
	assert!(validate(&entry).is_err());
	entry
		.config
		.as_object_mut()
		.unwrap()
		.remove("max_output_tokens");
	assert!(validate(&entry).is_err());
}

#[test]
fn registry_accepts_openrouter_and_rejects_unknown_providers() {
	let mut entry: Entry = serde_json::from_value(json!({
		"id":"router-model","version":"1.0.0","kind":"model",
		"name":{"en":"Router model"},"description":{"en":"Test model"},
		"config":config("openrouter","https://openrouter.ai/api/v1".into())
	}))
	.unwrap();
	validate(&entry).unwrap();
	for unsupported in ["openai", "anthropic", "unsupported"] {
		entry.config["provider"] = json!(unsupported);
		assert!(validate(&entry).is_err());
		assert!(
			provider(
				reqwest::Client::new(),
				config(unsupported, "http://localhost".into())
			)
			.is_err()
		);
	}
}

#[tokio::test]
async fn openrouter_enforces_zdr_and_preserves_reasoning_tools_and_usage() {
	let (tx, mut received) = tokio::sync::mpsc::unbounded_channel();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/api/v1/", listener.local_addr().unwrap());
	let app = Router::new().route(
		"/api/v1/chat/completions",
		post(move |Json(body): Json<Value>| {
			let tx = tx.clone();
			async move {
				tx.send(body.clone()).unwrap();
				let choice = if body["tools"].is_array() {
					json!({"finish_reason":"tool_calls","message":{"content":null,"tool_calls":[{
						"id":"call-1","type":"function","function":{"name":"read","arguments":"{\"path\":\"notes\"}"}
					}]}})
				} else {
					json!({"finish_reason":"stop","message":{"content":"Completed"}})
				};
				Json(
					json!({"choices":[choice],"usage":{"prompt_tokens":12,"completion_tokens":7,"cost":0.01}}),
				)
			}
		}),
	);
	let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
	let client = reqwest::Client::builder()
		.timeout(Duration::from_secs(5))
		.build()
		.unwrap();
	for effort in [
		None,
		Some(aidash::registry::ReasoningEffort::High),
		Some(aidash::registry::ReasoningEffort::None),
	] {
		let mut model_config = config("openrouter", endpoint.clone());
		model_config.reasoning_effort = effort;
		let max_output_tokens = model_config.output_token_limit();
		let model = provider(client.clone(), model_config).unwrap();
		for with_tools in [true, false] {
			let response = model
				.infer(ModelRequest {
					instructions: "Follow the task".into(),
					context: json!({"task":"Read notes"}),
					tools: if with_tools {
						vec![ToolSpec {
							name: "read".into(),
							description: "Read notes".into(),
							parameters: json!({"type":"object","properties":{"path":{"type":"string"}}}),
						}]
					} else {
						vec![]
					},
					max_output_tokens,
				})
				.await
				.unwrap();
			let request = received.recv().await.unwrap();
			assert_eq!(request["model"], "vendor/fixture-model");
			assert_eq!(request["messages"][0]["content"], "Follow the task");
			assert_eq!(
				serde_json::from_str::<Value>(request["messages"][1]["content"].as_str().unwrap())
					.unwrap(),
				json!({"task":"Read notes"})
			);
			assert_eq!(request["max_tokens"], 65536);
			assert!(request.get("max_completion_tokens").is_none());
			assert_eq!(request["provider"]["zdr"], true);
			assert_eq!(request["provider"]["require_parameters"], true);
			if let Some(effort) = effort {
				assert_eq!(request["reasoning"]["effort"], json!(effort));
			} else {
				assert!(request.get("reasoning").is_none());
			}
			assert_eq!((response.input_tokens, response.output_tokens), (12, 7));
			if with_tools {
				assert_eq!(request["tools"][0]["function"]["name"], "read");
				assert_eq!(response.tool_calls.len(), 1);
				assert_eq!(response.tool_calls[0].arguments, json!({"path":"notes"}));
			} else {
				assert!(request.get("tools").is_none());
				assert_eq!(response.text, "Completed");
				assert!(response.tool_calls.is_empty());
			}
		}
	}
	server.abort();
}

#[test]
fn refunds_require_complete_usage() {
	let incomplete = json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"ok"}}],"usage":{"completion_tokens":1}});
	assert!(
		!aidash::provider::parse_openai(incomplete)
			.unwrap()
			.usage_complete
	);
}

#[test]
fn malformed_arguments_and_inconsistent_stop_reasons_are_retryable() {
	use aidash::{Error, provider::parse_openai};
	for message in [
		json!({"content":"text"}),
		json!({"tool_calls":[{"id":"one","function":{"name":"tool","arguments":"{"}}]}),
	] {
		assert!(matches!(
			parse_openai(json!({"choices":[{"finish_reason":"tool_calls","message":message}]})),
			Err(Error::External(_))
		));
	}
}

#[test]
fn registry_and_transport_configs_reject_misspelled_optional_fields() {
	use aidash::{registry::AgentConfig, tool::ToolConfig};
	let mut model =
		serde_json::to_value(config("openrouter", "http://localhost/v1".into())).unwrap();
	model["credential_en"] = json!("AIDASH_SECRET_MISSING");
	assert!(serde_json::from_value::<ModelConfig>(model).is_err());
	assert!(
		serde_json::from_value::<AgentConfig>(
			json!({"model":{"id":"m","version":"1.0.0"},"instructions":"test","tool":[]})
		)
		.is_err()
	);
	assert!(serde_json::from_value::<ToolConfig>(json!({"transport":"http","endpoint":"http://localhost","replay":"unsafe","credential_en":"typo"})).is_err());
	assert!(serde_json::from_value::<Entry>(json!({"id":"a","version":"1.0.0","kind":"skill","name":{"en":"a"},"description":{},"capabilty":[]})).is_err());
}

#[test]
fn reasoning_effort_is_optional_and_rejects_unknown_values() {
	let mut value =
		serde_json::to_value(config("openrouter", "http://localhost/v1".into())).unwrap();
	value.as_object_mut().unwrap().remove("reasoning_effort");
	assert!(
		serde_json::from_value::<ModelConfig>(value.clone())
			.unwrap()
			.reasoning_effort
			.is_none()
	);
	value["reasoning_effort"] = json!("ultra");
	assert!(serde_json::from_value::<ModelConfig>(value).is_err());
}

#[tokio::test]
async fn unavailable_zdr_endpoint_does_not_retry_without_zdr() {
	use axum::http::StatusCode;
	let (tx, mut received) = tokio::sync::mpsc::unbounded_channel();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let app = Router::new().route(
		"/chat/completions",
		post(move |Json(body): Json<Value>| {
			let tx = tx.clone();
			async move {
				tx.send(body).unwrap();
				(
					StatusCode::NOT_FOUND,
					Json(json!({"error":"No ZDR endpoints"})),
				)
			}
		}),
	);
	let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
	let model = provider(reqwest::Client::new(), config("openrouter", endpoint)).unwrap();
	assert!(
		model
			.infer(ModelRequest {
				instructions: "test".into(),
				context: json!({}),
				tools: vec![],
				max_output_tokens: 512
			})
			.await
			.is_err()
	);
	assert_eq!(received.recv().await.unwrap()["provider"]["zdr"], true);
	assert!(received.try_recv().is_err());
	server.abort();
}
