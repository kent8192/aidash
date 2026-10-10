use super::*;
#[rstest::rstest]
fn media_reservation_covers_complete_request_growth() {
	let mut request = ModelRequest {
		instructions: "Inspect the media".into(),
		context: json!({"history":"quoted \\\"text\\\" and 日本語"}).into(),
		tools: vec![],
		max_output_tokens: 512,
		response_format: None,
		content_parts: vec![],
		cache_scope: None,
	};
	let without_media = request.estimated_total_tokens();
	request.content_parts = vec![
		ContentPart::Text("attachment: \\\"sample\\\"".into()),
		ContentPart::Image {
			media_type: "image/png".into(),
			bytes: vec![0; 8192],
		},
		ContentPart::Audio {
			format: "wav".into(),
			bytes: vec![0; 32_768],
		},
	];
	assert!(
		request.estimated_total_tokens()
			<= without_media.saturating_add(ModelRequest::content_parts_reservation(
				&request.content_parts
			))
	);
}

#[rstest::rstest]
fn parses_openrouter_tool_calls() {
	let result = parse_openai(json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"one","function":{"name":"search","arguments":"{\"q\":\"Rust\"}"}}]}}]})).unwrap();
	assert_eq!(result.tool_calls[0].arguments, json!({"q":"Rust"}));
}
#[rstest::rstest]
fn whitespace_only_tool_call_content_is_not_a_workspace_message() {
	let result = parse_openai(json!({"choices":[{"finish_reason":"tool_calls","message":{"content":" \n\t ","tool_calls":[{"id":"one","function":{"name":"search","arguments":"{}"}}]}}]})).unwrap();
	assert!(result.text.is_empty());
	assert_eq!(result.tool_calls.len(), 1);
	assert!(
		parse_openai(json!({"choices":[{"finish_reason":"stop","message":{"content":" \n\t "}}]}))
			.is_err()
	);
}
#[rstest::rstest]
fn truncation_cannot_complete_a_task() {
	assert!(matches!(
		parse_openai(
			json!({"choices":[{"finish_reason":"length","message":{"content":"partial"}}]})
		),
		Err(Error::TerminalResponse(Failure::OutputTruncated, _))
	));
}
#[rstest::rstest]
fn a_terminal_response_keeps_its_usage_for_settlement() {
	let error = parse_openai(json!({
		"choices":[{"finish_reason":"length","message":{"content":"partial"}}],
		"usage":{"prompt_tokens":1200,"completion_tokens":256}
	}))
	.unwrap_err();
	let usage = error.terminal_usage().unwrap();
	assert_eq!((usage.input_tokens, usage.output_tokens), (1200, 256));
	assert!(usage.usage_complete);
	assert!(usage.text.is_empty() && usage.tool_calls.is_empty());
	assert!(matches!(
		error.settled(),
		Error::Context(Failure::OutputTruncated)
	));
}
#[rstest::rstest]
#[case::length("length", Failure::OutputTruncated)]
#[case::content_filter("content_filter", Failure::Refused)]
fn truncated_or_filtered_output_never_yields_tool_calls(
	#[case] finish_reason: &str,
	#[case] expected: Failure,
) {
	let result = parse_openai(json!({"choices":[{"finish_reason":finish_reason,"message":{
		"tool_calls":[{"id":"one","function":{"name":"search","arguments":"{}"}}]}}]}));
	assert!(matches!(result, Err(Error::TerminalResponse(actual, _)) if actual == expected));
}
#[rstest::rstest]
#[case::stop("stop")]
#[case::tool_calls("tool_calls")]
fn a_refusal_is_typed_whatever_the_finish_reason(#[case] finish_reason: &str) {
	let result = parse_openai(json!({"choices":[{"finish_reason":finish_reason,"message":{
		"refusal":"I can't help with that",
		"tool_calls":[{"id":"one","function":{"name":"search","arguments":"{}"}}]}}]}));
	assert!(matches!(
		result,
		Err(Error::TerminalResponse(Failure::Refused, _))
	));
}
#[rstest::rstest]
fn other_unexpected_finish_reasons_remain_external() {
	assert!(matches!(
		parse_openai(json!({"choices":[{"finish_reason":"error","message":{"content":"x"}}]})),
		Err(Error::External(_))
	));
}
#[rstest::rstest]
#[case::openrouter(400, json!({"error":{"code":400,"message":"This endpoint's maximum context length is 8192 tokens. However, you requested about 9001 tokens (8000 of text input, 1001 in the output). Please reduce the length of either one."}}))]
#[case::code(400, json!({"error":{"code":"context_length_exceeded","message":"Input is too large"}}))]
#[case::message(400, json!({"error":{"message":"context_length_exceeded"}}))]
#[case::anthropic(400, json!({"error":{"code":400,"message":"Provider returned error","metadata":{"raw":"prompt is too long: 210000 tokens > 200000 maximum"}}}))]
#[case::payload_too_large(413, json!({"error":{"message":"prompt is too long"}}))]
fn provider_proven_overflow_is_typed(#[case] status: u16, #[case] body: Value) {
	for byok in [false, true] {
		assert!(matches!(
			rejection(status, Some(&body), byok),
			Error::ContextOverflow
		));
	}
}
#[rstest::rstest]
#[case::media(Some(json!({"error":{"message":"Audio exceeds the provider limit"}})), "Audio exceeds the provider limit")]
#[case::entity(Some(json!({"error":{"message":"Request Entity Too Large"}})), "upstream rejected the request")]
#[case::unreadable(None, "upstream rejected the request")]
fn payload_too_large_without_proof_is_not_an_overflow(
	#[case] body: Option<Value>,
	#[case] expected: &str,
) {
	assert!(matches!(
		rejection(413, body.as_ref(), false),
		Error::ProviderRejected { status: 413, reason } if reason == expected
	));
}
#[rstest::rstest]
#[case::invalid(400, Some(json!({"error":{"message":"Invalid tool schema: secret detail"}})))]
#[case::unreadable(400, None)]
#[case::overflow_wording_on_other_status(401, Some(json!({"error":{"message":"maximum context length is 10 tokens"}})))]
fn unrelated_rejections_stay_provider_rejected_without_the_body(
	#[case] status: u16,
	#[case] body: Option<Value>,
) {
	let error = rejection(status, body.as_ref(), false);
	assert!(
		matches!(&error, Error::ProviderRejected { status: actual, reason } if *actual == status && reason == "upstream rejected the request"),
		"{error:?}"
	);
	assert!(!error.to_string().contains("secret"));
}
#[rstest::rstest]
fn byok_chat_keeps_broker_capability_failures_typed() {
	let body = json!({"error":{"code":"capability_expired","message":"maximum context length"}});
	assert!(matches!(
		rejection(401, Some(&body), true),
		Error::Invalid(message) if message.contains("capability_expired")
	));
	assert!(matches!(
		rejection(401, Some(&body), false),
		Error::ProviderRejected { status: 401, .. }
	));
}
#[rstest::rstest]
#[case::text(vec![])]
#[case::media(vec!["google-vertex".to_owned()])]
fn every_chat_request_disables_openrouter_transforms(#[case] routes: Vec<String>) {
	let config: ModelConfig = serde_json::from_value(json!({
		"provider":"openrouter","model_id":"vendor/model","endpoint":"https://openrouter.ai/api/v1",
		"credential_env":null,"context_window":8192,"modalities":["text"],"cost":{}
	}))
	.unwrap();
	let request = ModelRequest {
		instructions: "Answer".into(),
		context: json!({"history":[]}).into(),
		tools: vec![],
		max_output_tokens: 256,
		response_format: None,
		content_parts: vec![],
		cache_scope: None,
	};
	let body = request_body(&config, &request, routes, None);
	assert_eq!(body["transforms"], json!([]));
	assert_eq!(body["model"], json!("vendor/model"));
	assert_eq!(body["provider"]["zdr"], json!(true));
}

mod cache_salt {
	use super::*;
	use aidash_domain::{
		projection::CacheScope,
		provider::{ModelContext, OrderedContext},
	};
	use axum::{Json, Router, routing::post};

	const SECRET_ONE: &str = "fixture-cache-salt-secret-one";
	const SECRET_TWO: &str = "fixture-cache-salt-secret-two";

	struct NoCredentials;
	impl aidash_application::ports::Credentials for NoCredentials {
		fn resolve(&self, _: &str) -> aidash_application::Result<String> {
			Err(aidash_application::Error::Invalid(
				"fixture models have no credential".into(),
			))
		}
	}

	struct TestServer(tokio::task::JoinHandle<()>);
	impl Drop for TestServer {
		fn drop(&mut self) {
			self.0.abort();
		}
	}

	/// An OpenRouter stand-in that records every completion body it receives.
	struct Upstream {
		endpoint: String,
		bodies: tokio::sync::mpsc::UnboundedReceiver<Value>,
		_server: TestServer,
	}

	impl Upstream {
		async fn start() -> Self {
			let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
			let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
			let (seen, bodies) = tokio::sync::mpsc::unbounded_channel();
			let app = Router::new().route(
				"/v1/chat/completions",
				post(move |Json(body): Json<Value>| {
					let seen = seen.clone();
					async move {
						seen.send(body).unwrap();
						Json(json!({"choices":[{"finish_reason":"stop","message":{"content":"done"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
					}
				}),
			);
			let server = TestServer(tokio::spawn(async move {
				axum::serve(listener, app).await.unwrap()
			}));
			Self {
				endpoint,
				bodies,
				_server: server,
			}
		}

		fn provider(&self, keys: Option<CacheSaltKeys>) -> Arc<dyn ModelProvider> {
			let config: ModelConfig = serde_json::from_value(json!({
				"provider": "openrouter",
				"model_id": "fixture",
				"endpoint": self.endpoint,
				"credential_env": null,
				"context_window": 100_000,
				"modalities": ["text"],
				"cost": {}
			}))
			.unwrap();
			salted_provider(
				reqwest::Client::new(),
				config,
				Arc::new(aidash_application::provider_access::EnvironmentAccess {
					credentials: Arc::new(NoCredentials),
				}),
				Default::default(),
				keys,
			)
			.unwrap()
		}

		/// Every body received so far, in arrival order.
		fn bodies(&mut self) -> Vec<Value> {
			let mut bodies = Vec::new();
			while let Ok(body) = self.bodies.try_recv() {
				bodies.push(body);
			}
			bodies
		}
	}

	fn keys() -> CacheSaltKeys {
		CacheSaltKeys::new(
			&[
				CacheSaltKey {
					version: 1,
					secret: SECRET_ONE.into(),
				},
				CacheSaltKey {
					version: 2,
					secret: SECRET_TWO.into(),
				},
			],
			2,
		)
		.unwrap()
	}

	fn scope(tenant: &str, key_version: u32) -> CacheScope {
		CacheScope {
			tenant: tenant.into(),
			key_version,
		}
	}

	fn legacy() -> ModelRequest {
		ModelRequest {
			instructions: "Shared instructions".into(),
			context: json!({"task":{"title":"Summarize"}}).into(),
			tools: vec![],
			max_output_tokens: 64,
			response_format: None,
			content_parts: vec![],
			cache_scope: None,
		}
	}

	fn ordered(scope: CacheScope) -> ModelRequest {
		ModelRequest {
			context: ModelContext::Ordered(OrderedContext {
				stable: r#"{"identity":"fixture"}"#.into(),
				volatile: r#"{"workspace":{}}"#.into(),
			}),
			cache_scope: Some(scope),
			..legacy()
		}
	}

	fn system(body: &Value) -> &str {
		body["messages"][0]["content"].as_str().unwrap()
	}

	fn first_line(body: &Value) -> &str {
		system(body).split_inclusive('\n').next().unwrap()
	}

	#[rstest::rstest]
	#[tokio::test]
	async fn legacy_requests_are_sent_without_a_salt() {
		let mut upstream = Upstream::start().await;
		let request = legacy();
		for keys in [Some(keys()), None] {
			upstream
				.provider(keys)
				.infer(request.clone())
				.await
				.unwrap();
		}
		let bodies = upstream.bodies();
		assert_eq!(bodies.len(), 2);
		for body in &bodies {
			assert_eq!(body["messages"], request.input_body()["messages"]);
			assert_eq!(system(body), "Shared instructions");
		}
	}

	#[rstest::rstest]
	#[tokio::test]
	async fn ordered_requests_start_system_with_the_tenant_salt() {
		let mut upstream = Upstream::start().await;
		let request = ordered(scope("tenant-a", 1));
		upstream
			.provider(Some(keys()))
			.infer(request.clone())
			.await
			.unwrap();
		let bodies = upstream.bodies();
		let body = &bodies[0];
		// The ADR 0016 derivation: HMAC-SHA256(key secret, Tenant id), 128 bits.
		use hmac::Mac as _;
		let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(SECRET_ONE.as_bytes()).unwrap();
		mac.update(b"tenant-a");
		let digest = mac.finalize().into_bytes();
		let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
		assert_eq!(
			system(body),
			format!("aidash-cache-scope:v1:{hex}\nShared instructions")
		);
		let line = first_line(body);
		let digest = line
			.strip_prefix("aidash-cache-scope:v1:")
			.and_then(|rest| rest.strip_suffix('\n'))
			.unwrap();
		assert_eq!(digest.len(), 32);
		assert!(
			digest
				.bytes()
				.all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
		);
		// Only `system` changes; the user parts and tools are the request's own.
		assert_eq!(body["messages"][1], request.input_body()["messages"][1]);
	}

	#[rstest::rstest]
	#[tokio::test]
	async fn salts_separate_tenants_and_key_versions_but_repeat_for_the_same_scope() {
		let mut upstream = Upstream::start().await;
		let provider = upstream.provider(Some(keys()));
		for scope in [
			scope("tenant-a", 2),
			scope("tenant-b", 2),
			scope("tenant-a", 2),
			scope("tenant-a", 1),
		] {
			provider.infer(ordered(scope)).await.unwrap();
		}
		let bodies = upstream.bodies();
		let lines: Vec<&str> = bodies.iter().map(first_line).collect();
		assert_ne!(
			lines[0], lines[1],
			"identical instructions, different Tenants"
		);
		assert_eq!(lines[0], lines[2], "same Tenant and key");
		assert_ne!(lines[0], lines[3], "rotated key version");
		assert!(lines[0].starts_with("aidash-cache-scope:v2:"));
		assert!(lines[3].starts_with("aidash-cache-scope:v1:"));
	}

	#[rstest::rstest]
	#[case::unknown_version(Some(keys()), 3)]
	#[case::node_without_keys(None, 1)]
	#[tokio::test]
	async fn a_missing_key_version_fails_before_any_request(
		#[case] keys: Option<CacheSaltKeys>,
		#[case] version: u32,
	) {
		let mut upstream = Upstream::start().await;
		let error = upstream
			.provider(keys)
			.infer(ordered(scope("tenant-a", version)))
			.await
			.unwrap_err();
		assert!(matches!(error, Error::Invalid(_)), "{error:?}");
		assert!(upstream.bodies().is_empty());
	}

	#[rstest::rstest]
	fn debug_output_never_reveals_the_secret_or_the_salt() {
		let keys = keys();
		let line = keys.line(&scope("tenant-a", 2)).unwrap();
		let digest = line.trim_end().rsplit(':').next().unwrap().to_owned();
		let request = ordered(scope("tenant-a", 2));
		let rendered = [
			format!("{keys:?}"),
			format!(
				"{:?}",
				CacheSaltKey {
					version: 2,
					secret: SECRET_TWO.into()
				}
			),
			format!("{request:?}"),
			format!("{:?}", keys.line(&scope("tenant-a", 9)).unwrap_err()),
		];
		for text in rendered {
			for hidden in [SECRET_ONE, SECRET_TWO, digest.as_str()] {
				assert!(!text.contains(hidden), "{text}");
			}
		}
	}

	#[rstest::rstest]
	#[case::empty_secret(&[(1, "  ")], 1)]
	#[case::current_not_configured(&[(1, SECRET_ONE)], 2)]
	#[case::duplicate_version(&[(1, SECRET_ONE), (1, SECRET_TWO)], 1)]
	fn invalid_key_settings_are_rejected(#[case] keys: &[(u32, &str)], #[case] current: u32) {
		let keys: Vec<CacheSaltKey> = keys
			.iter()
			.map(|(version, secret)| CacheSaltKey {
				version: *version,
				secret: (*secret).into(),
			})
			.collect();
		let error = CacheSaltKeys::new(&keys, current).unwrap_err();
		assert!(!format!("{error:?}").contains(SECRET_ONE));
	}
}
