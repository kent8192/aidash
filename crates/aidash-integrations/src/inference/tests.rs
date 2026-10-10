use super::*;
#[rstest::rstest]
fn media_reservation_covers_complete_request_growth() {
	let mut request = ModelRequest {
		instructions: "Inspect the media".into(),
		context: json!({"history":"quoted \\\"text\\\" and 日本語"}).into(),
		tools: vec![],
		max_output_tokens: 512,
		content_parts: vec![],
		cache_scope: None,
		cache_breakpoints: false,
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
	let result = parse(json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"one","function":{"name":"search","arguments":"{\"q\":\"Rust\"}"}}]}}]})).unwrap();
	assert_eq!(result.tool_calls[0].arguments, json!({"q":"Rust"}));
}
#[rstest::rstest]
fn whitespace_only_tool_call_content_is_not_a_workspace_message() {
	let result = parse(json!({"choices":[{"finish_reason":"tool_calls","message":{"content":" \n\t ","tool_calls":[{"id":"one","function":{"name":"search","arguments":"{}"}}]}}]})).unwrap();
	assert!(result.text.is_empty());
	assert_eq!(result.tool_calls.len(), 1);
	assert!(
		parse(json!({"choices":[{"finish_reason":"stop","message":{"content":" \n\t "}}]}))
			.is_err()
	);
}
#[rstest::rstest]
fn truncation_cannot_complete_a_task() {
	assert!(
		parse(json!({"choices":[{"finish_reason":"length","message":{"content":"partial"}}]}))
			.is_err()
	);
}

fn parse(value: Value) -> Result<ModelResponse> {
	parse_openai(value.to_string().as_bytes())
}

fn completion(usage: Value) -> Value {
	json!({"choices":[{"finish_reason":"stop","message":{"content":"done"}}],"usage":usage})
}

#[rstest::rstest]
#[case::complete(
	json!({"prompt_tokens":120,"completion_tokens":30,"total_tokens":150}),
	ReportedUsage { input_tokens: Some(120), output_tokens: Some(30), ..Default::default() },
	(120, 30, true)
)]
#[case::missing_prompt(
	json!({"completion_tokens":30}),
	ReportedUsage { output_tokens: Some(30), ..Default::default() },
	(0, 30, false)
)]
#[case::missing_completion(
	json!({"prompt_tokens":120}),
	ReportedUsage { input_tokens: Some(120), ..Default::default() },
	(120, 0, false)
)]
#[case::invalid_counts(
	json!({"prompt_tokens":-1,"completion_tokens":"30","prompt_tokens_details":{"cached_tokens":1.5}}),
	ReportedUsage::default(),
	(0, 0, false)
)]
#[case::cache_read_and_write(
	json!({"prompt_tokens":2000,"completion_tokens":40,"prompt_tokens_details":{"cached_tokens":1500,"cache_write_tokens":400},"completion_tokens_details":{"reasoning_tokens":12}}),
	ReportedUsage { input_tokens: Some(2000), output_tokens: Some(40), cache_read_tokens: Some(1500), cache_write_tokens: Some(400), reasoning_tokens: Some(12), cost: None },
	(2000, 40, true)
)]
#[case::cache_absent(
	json!({"prompt_tokens":2000,"completion_tokens":40,"prompt_tokens_details":{},"completion_tokens_details":null}),
	ReportedUsage { input_tokens: Some(2000), output_tokens: Some(40), ..Default::default() },
	(2000, 40, true)
)]
#[case::cost_present(
	json!({"prompt_tokens":10,"completion_tokens":5,"cost":0.0001234,"cost_details":{"upstream_inference_cost":0.0001}}),
	ReportedUsage { input_tokens: Some(10), output_tokens: Some(5), cost: Some(ProviderCost { nanocredits: 123_400, upstream_nanocredits: Some(100_000) }), ..Default::default() },
	(10, 5, true)
)]
#[case::cost_absent(
	json!({"prompt_tokens":10,"completion_tokens":5,"cost_details":{"upstream_inference_cost":0.0001}}),
	ReportedUsage { input_tokens: Some(10), output_tokens: Some(5), ..Default::default() },
	(10, 5, true)
)]
#[case::cost_exponent(
	json!({"prompt_tokens":10,"completion_tokens":5,"cost":1.5e-7,"cost_details":{"upstream_inference_cost":null}}),
	ReportedUsage { input_tokens: Some(10), output_tokens: Some(5), cost: Some(ProviderCost { nanocredits: 150, upstream_nanocredits: None }), ..Default::default() },
	(10, 5, true)
)]
#[case::cost_invalid(
	json!({"prompt_tokens":10,"completion_tokens":5,"cost":"0.1"}),
	ReportedUsage { input_tokens: Some(10), output_tokens: Some(5), ..Default::default() },
	(10, 5, true)
)]
fn openrouter_usage_keeps_unknown_fields_absent(
	#[case] usage: Value,
	#[case] reported: ReportedUsage,
	#[case] legacy: (u64, u64, bool),
) {
	let result = parse(completion(usage)).unwrap();
	assert_eq!(result.reported, reported);
	assert_eq!(
		(
			result.input_tokens,
			result.output_tokens,
			result.usage_complete
		),
		legacy
	);
}

#[rstest::rstest]
fn missing_usage_object_is_unknown() {
	let result =
		parse(json!({"choices":[{"finish_reason":"stop","message":{"content":"done"}}]})).unwrap();
	assert_eq!(result.reported, ReportedUsage::default());
	assert!(!result.usage_complete);
}

/// Costs keep their raw decimal text: rounding through `f64` first would turn
/// `0.1234567890000000000000000001` into `0.123456789` and understate the cost.
#[rstest::rstest]
#[case::cost_beyond_f64_precision(
	r#"{"choices":[{"finish_reason":"stop","message":{"content":"done"}}],"usage":{"prompt_tokens":1,"completion_tokens":1,"cost":0.1234567890000000000000000001,"cost_details":{"upstream_inference_cost":1.0000000000000000000000000001e-9}}}"#,
	Some(ProviderCost { nanocredits: 123_456_790, upstream_nanocredits: Some(2) })
)]
#[case::unexpected_cost_shape(
	r#"{"choices":[{"finish_reason":"stop","message":{"content":"done"}}],"usage":{"prompt_tokens":1,"completion_tokens":1,"cost":{"total":1},"cost_details":"none"}}"#,
	None
)]
fn openrouter_cost_uses_the_raw_decimal(#[case] body: &str, #[case] cost: Option<ProviderCost>) {
	let result = parse_openai(body.as_bytes()).unwrap();
	assert_eq!(result.reported.cost, cost);
	assert!(result.usage_complete);
}

/// Usage shape OpenRouter returns for an explicit `cache_control` request on an
/// Anthropic route: the cache counts are breakdowns of `prompt_tokens`.
#[rstest::rstest]
#[case::cache_write(json!({"prompt_tokens":4120,"completion_tokens":96,"total_tokens":4216,"cost":0.0277,"prompt_tokens_details":{"cached_tokens":0,"cache_write_tokens":4096},"completion_tokens_details":{"reasoning_tokens":0},"cost_details":{"upstream_inference_cost":0.0277}}), 4216)]
#[case::cache_read(json!({"prompt_tokens":4120,"completion_tokens":88,"total_tokens":4208,"cost":0.00268,"prompt_tokens_details":{"cached_tokens":4096,"cache_write_tokens":0},"completion_tokens_details":{"reasoning_tokens":0},"cost_details":{"upstream_inference_cost":0.00268}}), 4208)]
fn explicit_cache_settlement_charges_prompt_and_completion_once(
	#[case] usage: Value,
	#[case] total: i64,
) {
	let response = parse(completion(usage)).unwrap();
	let accounting = aidash_domain::generation::inference::accounting(8192, &response);
	assert_eq!(accounting.reported, Some(total));
	assert_eq!(accounting.refund, 8192 - total);
	assert!(!accounting.exceeded);
	assert_eq!(
		aidash_domain::generation::inference::complete_reported_usage(&response),
		Some(total)
	);
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
			self.route(keys, "fixture", None)
		}

		/// A provider for `model_id` declaring `cache_mode`, if any.
		fn route(
			&self,
			keys: Option<CacheSaltKeys>,
			model_id: &str,
			cache_mode: Option<&str>,
		) -> Arc<dyn ModelProvider> {
			let mut config = json!({
				"provider": "openrouter",
				"model_id": model_id,
				"endpoint": self.endpoint,
				"credential_env": null,
				"context_window": 100_000,
				"modalities": ["text"],
				"cost": {}
			});
			if let Some(mode) = cache_mode {
				config["cache_mode"] = json!(mode);
			}
			let config: ModelConfig = serde_json::from_value(config).unwrap();
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
			content_parts: vec![],
			cache_scope: None,
			cache_breakpoints: false,
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
	#[tokio::test]
	async fn explicit_routes_receive_breakpoints_after_the_tenant_salt() {
		// Arrange
		let mut upstream = Upstream::start().await;
		let provider = upstream.route(Some(keys()), "anthropic/claude-fixture", Some("explicit"));
		let marked = ModelRequest {
			cache_breakpoints: true,
			..ordered(scope("tenant-a", 1))
		};
		let expected_line = keys().line(&scope("tenant-a", 1)).unwrap();
		// Act
		provider.infer(marked.clone()).await.unwrap();
		provider.infer(ordered(scope("tenant-a", 1))).await.unwrap();
		// Assert
		let bodies = upstream.bodies();
		let ephemeral = json!({"type":"ephemeral"});
		assert_eq!(
			bodies[0]["messages"][0]["content"],
			json!([{"type":"text","text":format!("{expected_line}Shared instructions"),"cache_control":ephemeral}])
		);
		let parts = bodies[0]["messages"][1]["content"].as_array().unwrap();
		assert_eq!(
			parts[0]["cache_control"], ephemeral,
			"the Stable Prefix part"
		);
		assert!(parts[1].get("cache_control").is_none(), "the volatile part");
		assert_eq!(bodies[0]["messages"][1], marked.input_body()["messages"][1]);
		assert_eq!(bodies[0]["provider"]["zdr"], true);
		assert!(!bodies[1].to_string().contains("cache_control"));
		assert_eq!(
			first_line(&bodies[1]),
			expected_line,
			"the same Tenant salt with or without breakpoints"
		);
	}

	#[rstest::rstest]
	#[case::undeclared("anthropic/claude-fixture", None)]
	#[case::automatic("anthropic/claude-fixture", Some("automatic"))]
	#[case::outside_the_allowlist("openai/gpt-fixture", Some("explicit"))]
	#[tokio::test]
	async fn other_routes_never_receive_breakpoints(
		#[case] model_id: &str,
		#[case] cache_mode: Option<&str>,
	) {
		// Arrange
		let mut upstream = Upstream::start().await;
		let marked = ModelRequest {
			cache_breakpoints: true,
			..ordered(scope("tenant-a", 1))
		};
		// Act
		let error = upstream
			.route(Some(keys()), model_id, cache_mode)
			.infer(marked)
			.await
			.unwrap_err();
		// Assert
		assert!(
			matches!(&error, Error::Invalid(message) if message.contains("does not accept cache breakpoints")),
			"{error:?}"
		);
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
