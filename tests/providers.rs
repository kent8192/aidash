use aidash::{
	provider::{ContentPart, ModelRequest, ToolSpec, provider},
	registry::{Entry, MediaRouteEvidence, ModelConfig, validate},
};
use axum::{
	Json, Router,
	routing::{get, post},
};
use serde_json::{Value, json};
use std::time::Duration;

fn config(provider: &str, endpoint: String) -> ModelConfig {
	ModelConfig {
		provider: provider.into(),
		model_id: "vendor/fixture-model".into(),
		endpoint,
		credential_env: None,
		request_timeout_secs: None,
		reasoning_effort: None,
		context_window: 128000,
		max_output_tokens: Some(65536),
		modalities: vec!["text".into()],
		media_routes: vec![],
		cost: json!({}),
	}
}

#[rstest::rstest]
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

#[rstest::rstest]
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

#[rstest::rstest]
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

#[rstest::rstest]
fn media_admission_requires_a_current_route_covering_every_selected_format() {
	let mut model = config("openrouter", "https://openrouter.ai/api/v1".into());
	model.modalities = vec!["text".into(), "image".into(), "audio".into()];
	let route = MediaRouteEvidence {
		tag: "provider/exact".into(),
		formats: vec!["image/png".into(), "wav".into()],
		source: "bounded verification".into(),
		verified_at: chrono::Utc::now() - chrono::Duration::hours(1),
		expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
	};
	assert!(model.require_media_types(["image/png"]).is_err());
	model.media_routes.push(route);
	model
		.require_media_types(["image/png", "audio/wav"])
		.unwrap();
	assert!(
		model
			.require_media_types(["image/png", "audio/mpeg"])
			.is_err()
	);
	model.media_routes[0].expires_at = chrono::Utc::now() - chrono::Duration::seconds(1);
	assert!(model.require_media_types(["audio/wav"]).is_err());
}

#[rstest::rstest]
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
					content_parts: vec![],
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

#[rstest::rstest]
#[tokio::test]
async fn openrouter_sends_ordered_native_image_and_audio_parts() {
	let (tx, mut received) = tokio::sync::mpsc::unbounded_channel();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/api/v1", listener.local_addr().unwrap());
	let app = Router::new().route("/api/v1/chat/completions", post(move |Json(body): Json<Value>| {
		let tx = tx.clone();
		async move {
			tx.send(body).unwrap();
			Json(json!({"choices":[{"finish_reason":"stop","message":{"content":"I saw and heard the input"}}]}))
		}
	}))
	.route("/api/v1/models/vendor/fixture-model/endpoints", get(|| async {
		Json(json!({"data":{"architecture":{"input_modalities":["text","image","audio"]},"endpoints":[{"tag":"fixture/verified","context_length":128000}]}}))
	}))
	.route("/api/v1/endpoints/zdr", get(|| async {
		Json(json!({"data":[{"model_id":"vendor/fixture-model","tag":"fixture/verified"}]}))
	}));
	let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
	let mut model_config = config("openrouter", endpoint);
	model_config.modalities = vec!["text".into(), "image".into(), "audio".into()];
	model_config.media_routes.push(MediaRouteEvidence {
		tag: "fixture/verified".into(),
		formats: vec!["image/png".into(), "wav".into()],
		source: "bounded fixture verification".into(),
		verified_at: chrono::Utc::now() - chrono::Duration::hours(1),
		expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
	});
	let model = provider(reqwest::Client::new(), model_config).unwrap();
	let image = b"\x89PNG\r\n\x1a\nimage".to_vec();
	let audio = b"RIFF\0\0\0\0WAVEaudio".to_vec();
	let response = model
		.infer(ModelRequest {
			instructions: "Inspect the media".into(),
			context: json!({"run_message":"Describe the attachment"}),
			tools: vec![],
			max_output_tokens: 512,
			content_parts: vec![
				ContentPart::Text("first attachment".into()),
				ContentPart::Image {
					media_type: "image/png".into(),
					bytes: image,
				},
				ContentPart::Text("second attachment".into()),
				ContentPart::Audio {
					format: "wav".into(),
					bytes: audio,
				},
			],
		})
		.await
		.unwrap();
	assert_eq!(response.text, "I saw and heard the input");
	let body = received.recv().await.unwrap();
	let parts = body["messages"][1]["content"].as_array().unwrap();
	assert_eq!(
		parts
			.iter()
			.map(|part| part["type"].as_str().unwrap())
			.collect::<Vec<_>>(),
		["text", "text", "image_url", "text", "input_audio"]
	);
	assert!(
		parts[2]["image_url"]["url"]
			.as_str()
			.unwrap()
			.starts_with("data:image/png;base64,")
	);
	assert_eq!(parts[4]["input_audio"]["format"], "wav");
	assert_eq!(parts[4]["input_audio"]["data"], "UklGRgAAAABXQVZFYXVkaW8=");
	assert_eq!(body["provider"]["zdr"], true);
	assert_eq!(body["provider"]["only"], json!(["fixture/verified"]));
	server.abort();
}

#[rstest::rstest]
fn mp3_signature_accepts_crc_and_mpeg_25_layer_three_frames() {
	for second in [0xfa, 0xfb, 0xf2, 0xf3, 0xe2, 0xe3] {
		assert!(
			ContentPart::from_media("audio/mpeg", vec![0xff, second, 0x90, 0x64]).is_ok(),
			"valid MPEG header second byte {second:#x}"
		);
	}
	assert!(ContentPart::from_media("audio/mpeg", b"ID3fixture".to_vec()).is_ok());
	for invalid in [
		vec![0xff, 0xfe, 0x90, 0x64], // Layer I
		vec![0xff, 0xea, 0x90, 0x64], // reserved MPEG version
		vec![0xff, 0xfa, 0xf0, 0x64], // reserved bitrate
		vec![0xff, 0xfa, 0x9c, 0x64], // reserved sample rate
		vec![0xff, 0xfa, 0x90],
	] {
		assert!(ContentPart::from_media("audio/mpeg", invalid).is_err());
	}
}

#[rstest::rstest]
#[tokio::test]
async fn media_route_lookup_obeys_the_total_inference_deadline() {
	use aidash::Error;
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/api/v1", listener.local_addr().unwrap());
	let app = Router::new()
		.route(
			"/api/v1/models/vendor/fixture-model/endpoints",
			get(|| async {
				tokio::time::sleep(Duration::from_secs(5)).await;
				Json(
					json!({"data":{"architecture":{"input_modalities":["text","image"]},"endpoints":[]}}),
				)
			}),
		)
		.route(
			"/api/v1/endpoints/zdr",
			get(|| async { Json(json!({"data":[]})) }),
		);
	let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
	let mut model_config = config("openrouter", endpoint);
	model_config.request_timeout_secs = Some(1);
	model_config.modalities.push("image".into());
	model_config.media_routes.push(MediaRouteEvidence {
		tag: "fixture/verified".into(),
		formats: vec!["image/png".into()],
		source: "fixture verification".into(),
		verified_at: chrono::Utc::now() - chrono::Duration::hours(1),
		expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
	});
	let model = provider(reqwest::Client::new(), model_config).unwrap();
	let result = tokio::time::timeout(
		Duration::from_secs(3),
		model.infer(ModelRequest {
			instructions: String::new(),
			context: json!({}),
			tools: vec![],
			max_output_tokens: 128,
			content_parts: vec![ContentPart::Image {
				media_type: "image/png".into(),
				bytes: b"\x89PNG\r\n\x1a\nfixture".to_vec(),
			}],
		}),
	)
	.await
	.expect("the configured inference deadline was exceeded");
	assert!(
		matches!(result, Err(Error::External(message)) if message == "model inference timed out")
	);
	server.abort();
}

#[rstest::rstest]
fn refunds_require_complete_usage() {
	let incomplete = json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"ok"}}],"usage":{"completion_tokens":1}});
	assert!(
		!aidash::provider::parse_openai(incomplete)
			.unwrap()
			.usage_complete
	);
}

#[rstest::rstest]
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

#[rstest::rstest]
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

#[rstest::rstest]
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

#[rstest::rstest]
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
				max_output_tokens: 512,
				content_parts: vec![]
			})
			.await
			.is_err()
	);
	assert_eq!(received.recv().await.unwrap()["provider"]["zdr"], true);
	assert!(received.try_recv().is_err());
	server.abort();
}

#[rstest::rstest]
#[tokio::test]
async fn upstream_media_rejection_keeps_its_status_and_safe_reason() {
	use aidash::Error;
	use axum::http::StatusCode;
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let app = Router::new().route(
		"/chat/completions",
		post(|| async {
			(
				StatusCode::PAYLOAD_TOO_LARGE,
				Json(json!({"error":{"message":"Audio exceeds the provider limit"}})),
			)
		}),
	);
	let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
	let model = provider(reqwest::Client::new(), config("openrouter", endpoint)).unwrap();
	let error = model
		.infer(ModelRequest {
			instructions: "test".into(),
			context: json!({}),
			tools: vec![],
			max_output_tokens: 512,
			content_parts: vec![],
		})
		.await
		.unwrap_err();
	assert!(
		matches!(error, Error::ProviderRejected { status: 413, reason } if reason == "Audio exceeds the provider limit")
	);
	server.abort();
}

#[rstest::rstest]
#[tokio::test]
async fn upstream_errors_cannot_echo_unrecognized_media_or_secret_data() {
	use aidash::Error;
	use axum::http::StatusCode;
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let app = Router::new().route(
		"/chat/completions",
		post(|| async {
			(
				StatusCode::SERVICE_UNAVAILABLE,
				Json(
					json!({"error":{"message":"input_audio.data=U2Vuc2l0aXZlQnl0ZXM=; token=private"}}),
				),
			)
		}),
	);
	let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
	let model = provider(reqwest::Client::new(), config("openrouter", endpoint)).unwrap();
	let error = model
		.infer(ModelRequest {
			instructions: "test".into(),
			context: json!({}),
			tools: vec![],
			max_output_tokens: 512,
			content_parts: vec![],
		})
		.await
		.unwrap_err();
	assert!(matches!(
		error,
		Error::ProviderRejected { status: 503, reason }
			if reason == "upstream rejected the request"
	));
	server.abort();
}
