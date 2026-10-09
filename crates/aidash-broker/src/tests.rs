use super::*;
use aidash_capability::{InMemorySigner, TokenSigner, TokenSubject, mint};
use async_trait::async_trait;
use axum::{Json, routing::any};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tower::ServiceExt;
mod cloud;

const CANARY: &str = "canary-provider-key-never-in-errors-or-logs";
#[derive(Default)]
struct Logs(Mutex<Vec<String>>);
impl AuditSink for Logs {
	fn record(&self, audit: &Audit) {
		self.0
			.lock()
			.unwrap()
			.push(serde_json::to_string(audit).unwrap());
	}
}
#[derive(Default)]
struct Source {
	reads: AtomicUsize,
	unavailable: AtomicBool,
	secrets: Mutex<Vec<String>>,
}
#[async_trait]
impl KeyMaterialSource for Source {
	async fn access(&self, secret: &str, _: &str) -> Result<SecretString, KeyMaterialError> {
		self.reads.fetch_add(1, Ordering::SeqCst);
		self.secrets.lock().unwrap().push(secret.into());
		if self.unavailable.load(Ordering::SeqCst) {
			Err(KeyMaterialError::Unavailable)
		} else {
			Ok(CANARY.into())
		}
	}
}
#[derive(Default)]
struct ProviderState {
	calls: AtomicUsize,
	bearer: Mutex<Vec<String>>,
	paths: Mutex<Vec<String>>,
	finish: tokio::sync::Notify,
	completed: AtomicBool,
}
async fn upstream(State(state): State<Arc<ProviderState>>, request: Request) -> Response {
	state.calls.fetch_add(1, Ordering::SeqCst);
	state.bearer.lock().unwrap().push(
		request.headers()[header::AUTHORIZATION]
			.to_str()
			.unwrap()
			.into(),
	);
	let path = request.uri().path().to_owned();
	state.paths.lock().unwrap().push(path.clone());
	let body = to_bytes(request.into_body(), CHAT_REQUEST_LIMIT)
		.await
		.unwrap();
	let value: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
	match value.get("test").and_then(Value::as_str) {
		Some("error") => {
			return (
				StatusCode::BAD_REQUEST,
				Json(json!({"error":{"message":CANARY}})),
			)
				.into_response();
		}
		Some("audio") => {
			return (
				StatusCode::BAD_REQUEST,
				Json(json!({"error":{"message":format!("audio exceeds duration limit {CANARY}")}})),
			)
				.into_response();
		}
		Some("redirect") => {
			return Response::builder()
				.status(302)
				.header(header::LOCATION, "http://127.0.0.1:1/exfiltrate")
				.body(Body::empty())
				.unwrap();
		}
		Some("huge") => return Response::new(Body::from(vec![b'x'; 1_048_577])),
		_ => {}
	}
	if value.get("stream") == Some(&Value::Bool(true)) {
		let stream = async_stream::stream! {
			yield Ok::<_, std::io::Error>(Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"content\":\"early\"}}]}\n\n"));
			state.finish.notified().await;
			state.completed.store(true, Ordering::SeqCst);
			yield Ok(Bytes::from_static(b"data: {\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":2,\"total_tokens\":6}}\n\ndata: [DONE]\n\n"));
		};
		return Response::builder()
			.header(header::CONTENT_TYPE, "text/event-stream")
			.body(Body::from_stream(stream))
			.unwrap();
	}
	Json(match path.as_str() {
		"/api/v1/models/author/model/endpoints" => json!({"data":{"architecture":{"input_modalities":["text","image"]},"endpoints":[{"tag":"vendor/route","context_length":131072}]}}),
		"/api/v1/endpoints/zdr" => json!({"data":[{"model_id":"author/model","tag":"vendor/route"}]}),
		"/api/v1/embeddings" => json!({"model":"author/model","data":[{"index":0,"embedding":[1.0,0.0]}],"usage":{"prompt_tokens":4,"total_tokens":4}}),
		_ => json!({"choices":[{"finish_reason":"stop","message":{"content":"ok"}}],"usage":{"prompt_tokens":4,"completion_tokens":2,"total_tokens":6,"untrusted":CANARY}})
	}).into_response()
}
struct Fixture {
	broker: Arc<Broker>,
	signer: InMemorySigner,
	source: Arc<Source>,
	logs: Arc<Logs>,
	provider: Arc<ProviderState>,
	server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
	fn drop(&mut self) {
		self.server.abort();
	}
}
fn config() -> Config {
	Config {
		issuer: "worker".into(),
		audience: "environment".into(),
		byok_project_number: "1234".into(),
		secret_prefix: "aidash-develop-cred-".into(),
		requests_per_second: 100.0,
		burst: 100,
		inference_deadline: Duration::from_secs(5),
	}
}
impl Fixture {
	async fn new() -> Self {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let base = format!("http://{}/api/v1", listener.local_addr().unwrap());
		let provider = Arc::new(ProviderState::default());
		let router = Router::new()
			.fallback(any(upstream))
			.with_state(provider.clone());
		let server = tokio::spawn(async move {
			axum::serve(listener, router).await.unwrap();
		});
		let signer = InMemorySigner::new("kms/1".into(), [7; 32]);
		let mut keys = PublicKeys::default();
		keys.insert(signer.kid().into(), signer.public_key());
		let source = Arc::new(Source::default());
		let logs = Arc::new(Logs::default());
		let mut broker = Broker::new(config(), keys, source.clone(), logs.clone()).unwrap();
		// Only this private test module can replace the code-defined Provider Catalog.
		broker.catalog.insert("openrouter".into(), base);
		Self {
			broker: Arc::new(broker),
			signer,
			source,
			logs,
			provider,
			server,
		}
	}
	fn claims(&self) -> Claims {
		let now = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap()
			.as_secs();
		Claims {
			iss: "worker".into(),
			aud: "environment".into(),
			iat: now,
			exp: now + 60,
			jti: uuid::Uuid::new_v4(),
			kid: self.signer.kid().into(),
			tenant: "tenant-a".into(),
			provider: "openrouter".into(),
			credential: uuid::Uuid::from_u128(100),
			version: "1".into(),
			sub: TokenSubject::Run {
				run: "run-1".into(),
				call: uuid::Uuid::new_v4(),
			},
			ops: vec![Operation::Chat, Operation::Discovery, Operation::Embeddings],
			model: "author/model".into(),
			max_output_tokens: 10,
		}
	}
	async fn request(&self, claims: &Claims, method: &str, uri: &str, body: Value) -> Response {
		let token = mint(claims, &self.signer).await.unwrap();
		let body = if body.is_null() {
			Body::empty()
		} else {
			Body::from(body.to_string())
		};
		self.broker
			.clone()
			.router()
			.oneshot(
				Request::builder()
					.method(method)
					.uri(uri)
					.header(
						header::AUTHORIZATION,
						format!("Bearer {}", token.expose_secret()),
					)
					.header(header::HOST, "attacker.invalid")
					.body(body)
					.unwrap(),
			)
			.await
			.unwrap()
	}
}
fn chat() -> Value {
	json!({"model":"author/model","max_tokens":10,"provider":{"zdr":true}})
}
async fn json_body(response: Response) -> Value {
	serde_json::from_slice(
		&to_bytes(response.into_body(), 8 * 1024 * 1024)
			.await
			.unwrap(),
	)
	.unwrap()
}

#[tokio::test]
async fn discovery_accepts_validated_model_paths_and_rejects_other_signed_models() {
	let f = Fixture::new().await;
	for model in ["single-model", "author/model", "organization/family/model"] {
		aidash_domain::provider_credentials::validate_model_id(model).unwrap();
		let mut claims = f.claims();
		claims.model = model.into();
		let uri = format!("/api/v1/models/{model}/endpoints");
		let response = f.request(&claims, "GET", &uri, Value::Null).await;
		assert_eq!(response.status(), 200, "validated model {model}");
		json_body(response).await;
		assert_eq!(f.provider.paths.lock().unwrap().last(), Some(&uri));

		let calls = f.provider.calls.load(Ordering::SeqCst);
		let reads = f.source.reads.load(Ordering::SeqCst);
		let response = f
			.request(
				&claims,
				"GET",
				"/api/v1/models/other/model/endpoints",
				Value::Null,
			)
			.await;
		assert_eq!(response.status(), 403);
		assert_eq!(
			json_body(response).await["error"]["code"],
			"capability_model"
		);
		assert_eq!(f.provider.calls.load(Ordering::SeqCst), calls);
		assert_eq!(f.source.reads.load(Ordering::SeqCst), reads);
	}
}

#[tokio::test]
async fn discovery_without_a_model_fails_closed_before_key_lookup() {
	let f = Fixture::new().await;
	let response = f
		.request(&f.claims(), "GET", "/api/v1/models/endpoints", Value::Null)
		.await;
	assert_eq!(response.status(), 403);
	assert_eq!(
		json_body(response).await["error"]["code"],
		"capability_model"
	);
	assert_eq!(f.source.reads.load(Ordering::SeqCst), 0);
	assert_eq!(f.provider.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn chat_rejects_provider_executed_tools_and_plugins_but_preserves_function_tools() {
	let f = Fixture::new().await;
	for (field, value) in [
		("tools", json!([{"type":"openrouter:web_search"}])),
		("tools", json!([{"type":"openrouter:web_fetch"}])),
		(
			"tools",
			json!([{"type":"function"}, {"type":"openrouter:web_search"}]),
		),
		("tools", json!([{"type":"unknown"}])),
		("tools", json!([{}])),
		("tools", json!({"type":"function"})),
		("tools", Value::Null),
		("plugins", json!([{"id":"web"}])),
		("plugins", json!([])),
		("plugins", Value::Null),
	] {
		let mut body = chat();
		body[field] = value;
		let response = f
			.request(&f.claims(), "POST", "/api/v1/chat/completions", body)
			.await;
		assert_eq!(response.status(), 403, "unauthorized {field}");
		assert_eq!(
			json_body(response).await["error"]["code"],
			"capability_claim_violation"
		);
		assert_eq!(f.provider.calls.load(Ordering::SeqCst), 0);
		assert_eq!(f.source.reads.load(Ordering::SeqCst), 0);
	}
	for tools in [
		json!([]),
		json!([{"type":"function","function":{"name":"workspace_read","parameters":{"type":"object","properties":{}}}}]),
	] {
		let mut body = chat();
		body["tools"] = tools;
		let response = f
			.request(&f.claims(), "POST", "/api/v1/chat/completions", body)
			.await;
		assert_eq!(response.status(), 200);
		json_body(response).await;
	}
	assert_eq!(f.provider.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn chat_rejects_non_text_output_and_media_configuration_before_key_lookup() {
	let f = Fixture::new().await;
	for (field, value) in [
		("modalities", json!(["image"])),
		("modalities", json!(["text", "image"])),
		("modalities", json!(["audio"])),
		("modalities", json!(["text", "audio"])),
		("modalities", json!(["unknown"])),
		("modalities", json!([])),
		("modalities", json!("text")),
		("modalities", Value::Null),
		("image_config", json!({"aspect_ratio":"1:1"})),
		("image_config", Value::Null),
		("audio", json!({"voice":"alloy","format":"wav"})),
		("audio", Value::Null),
	] {
		let mut body = chat();
		body[field] = value;
		let response = f
			.request(&f.claims(), "POST", "/api/v1/chat/completions", body)
			.await;
		assert_eq!(response.status(), 403, "unauthorized {field}");
		assert_eq!(
			json_body(response).await["error"]["code"],
			"capability_claim_violation"
		);
		assert_eq!(f.provider.calls.load(Ordering::SeqCst), 0);
		assert_eq!(f.source.reads.load(Ordering::SeqCst), 0);
	}
	let mut body = chat();
	body["modalities"] = json!(["text"]);
	let response = f
		.request(&f.claims(), "POST", "/api/v1/chat/completions", body)
		.await;
	assert_eq!(response.status(), 200);
	json_body(response).await;
	assert_eq!(f.provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn chat_allows_only_the_default_service_tier_before_key_lookup() {
	let f = Fixture::new().await;
	for tier in [
		json!("priority"),
		json!("scale"),
		json!("fast"),
		json!("ultrafast"),
		json!("flex"),
		json!("auto"),
		Value::Null,
		json!(5),
	] {
		let mut body = chat();
		body["service_tier"] = tier;
		let response = f
			.request(&f.claims(), "POST", "/api/v1/chat/completions", body)
			.await;
		assert_eq!(response.status(), 403);
		assert_eq!(
			json_body(response).await["error"]["code"],
			"capability_claim_violation"
		);
		assert_eq!(f.source.reads.load(Ordering::SeqCst), 0);
		assert_eq!(f.provider.calls.load(Ordering::SeqCst), 0);
	}
	let mut body = chat();
	body["service_tier"] = json!("default");
	let response = f
		.request(&f.claims(), "POST", "/api/v1/chat/completions", body)
		.await;
	assert_eq!(response.status(), 200);
	json_body(response).await;
}

#[tokio::test]
async fn embeddings_authorize_one_string_input_before_key_lookup() {
	let f = Fixture::new().await;
	for input in [
		json!(["one", "two"]),
		json!(["one"]),
		json!([]),
		json!([1, 2]),
		json!(5),
		Value::Null,
	] {
		let body = json!({"model":"author/model", "provider":{"zdr":true}, "input":input});
		let response = f
			.request(&f.claims(), "POST", "/api/v1/embeddings", body)
			.await;
		assert_eq!(response.status(), 403);
		assert_eq!(
			json_body(response).await["error"]["code"],
			"capability_claim_violation"
		);
		assert_eq!(f.source.reads.load(Ordering::SeqCst), 0);
		assert_eq!(f.provider.calls.load(Ordering::SeqCst), 0);
	}
	let body = json!({"model":"author/model", "provider":{"zdr":true}});
	let response = f
		.request(&f.claims(), "POST", "/api/v1/embeddings", body)
		.await;
	assert_eq!(response.status(), 403);
	assert_eq!(
		json_body(response).await["error"]["code"],
		"capability_claim_violation"
	);
	assert_eq!(f.source.reads.load(Ordering::SeqCst), 0);
	assert_eq!(f.provider.calls.load(Ordering::SeqCst), 0);
	let body = json!({"model":"author/model", "provider":{"zdr":true}, "input":"one"});
	let response = f
		.request(&f.claims(), "POST", "/api/v1/embeddings", body)
		.await;
	assert_eq!(response.status(), 200);
	assert_eq!(
		json_body(response).await["data"].as_array().unwrap().len(),
		1
	);
}

#[tokio::test]
async fn maintenance_purposes_authorize_only_the_explicit_operation() {
	let f = Fixture::new().await;
	for purpose in [
		aidash_capability::Maintenance::MemoryIndexing,
		aidash_capability::Maintenance::MemoryRetention,
		aidash_capability::Maintenance::MemoryReflection,
		aidash_capability::Maintenance::MemoryRetrieval,
	] {
		for (operation, path, body, other_path, other_body) in [
			(
				Operation::Chat,
				"/api/v1/chat/completions",
				chat(),
				"/api/v1/embeddings",
				json!({"model":"author/model", "input":"test", "provider":{"zdr":true}}),
			),
			(
				Operation::Embeddings,
				"/api/v1/embeddings",
				json!({"model":"author/model", "input":"test", "provider":{"zdr":true}}),
				"/api/v1/chat/completions",
				chat(),
			),
		] {
			let mut c = f.claims();
			c.sub = TokenSubject::Maintenance {
				maintenance: purpose,
				tenant: c.tenant.clone(),
			};
			c.ops = vec![operation];
			let response = f.request(&c, "POST", path, body).await;
			assert_eq!(response.status(), 200);
			json_body(response).await;
			let response = f.request(&c, "POST", other_path, other_body).await;
			assert_eq!(response.status(), 403);
			assert_eq!(
				json_body(response).await["error"]["code"],
				"capability_operation"
			);
		}
	}
}

#[tokio::test]
async fn deterministic_capability_and_body_rejections_never_call_provider() {
	let f = Fixture::new().await;
	for (change, reason) in [
		(0, "audience"),
		(1, "expired"),
		(2, "tenant"),
		(3, "credential"),
		(4, "credential"),
		(5, "operation"),
		(6, "tenant"),
	] {
		let mut c = f.claims();
		match change {
			0 => c.aud = "other".into(),
			1 => {
				c.iat -= 61;
				c.exp -= 61;
			}
			2 => {
				c.sub = TokenSubject::Maintenance {
					maintenance: aidash_capability::Maintenance::MemoryIndexing,
					tenant: "other".into(),
				}
			}
			3 => c.credential = uuid::Uuid::nil(),
			4 => c.version = "latest".into(),
			5 => c.ops = vec![Operation::Embeddings],
			6 => c.provider = "https://attacker.invalid".into(),
			_ => unreachable!(),
		}
		let r = f
			.request(&c, "POST", "/api/v1/chat/completions", chat())
			.await;
		assert!(matches!(r.status().as_u16(), 401 | 403));
		assert_eq!(
			json_body(r).await["error"]["code"],
			format!("capability_{reason}")
		);
	}
	for (field, value, reason) in [
		("model", json!("other/model"), "model"),
		("models", json!(["author/model", "other/model"]), "model"),
		("models", json!(["author/model"]), "model"),
		("models", json!([]), "model"),
		("models", Value::Null, "model"),
		("max_tokens", json!(11), "claim_violation"),
		("max_tokens", Value::Null, "claim_violation"),
		("max_completion_tokens", json!(11), "claim_violation"),
		("max_completion_tokens", json!(10), "claim_violation"),
		("max_completion_tokens", Value::Null, "claim_violation"),
		("provider", json!({"zdr":false}), "claim_violation"),
	] {
		let mut body = chat();
		body[field] = value;
		let r = f
			.request(&f.claims(), "POST", "/api/v1/chat/completions", body)
			.await;
		assert_eq!(r.status(), 403);
		assert_eq!(
			json_body(r).await["error"]["code"],
			format!("capability_{reason}")
		);
	}
	for (signer, reason) in [
		(
			InMemorySigner::new("unknown".into(), [7; 32]),
			"unknown_kid",
		),
		(
			InMemorySigner::new("kms/1".into(), [8; 32]),
			"invalid_signature",
		),
	] {
		let mut c = f.claims();
		c.kid = signer.kid().into();
		let token = mint(&c, &signer).await.unwrap();
		let r = f
			.broker
			.clone()
			.router()
			.oneshot(
				Request::builder()
					.method("POST")
					.uri("/api/v1/chat/completions")
					.header(
						header::AUTHORIZATION,
						format!("Bearer {}", token.expose_secret()),
					)
					.body(Body::from(chat().to_string()))
					.unwrap(),
			)
			.await
			.unwrap();
		assert_eq!(r.status(), 401);
		assert_eq!(
			json_body(r).await["error"]["code"],
			format!("capability_{reason}")
		);
	}
	assert_eq!(f.provider.calls.load(Ordering::SeqCst), 0);
	assert_eq!(f.source.reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cannot_redirect_or_override_catalog_through_uri_or_headers() {
	let f = Fixture::new().await;
	for uri in [
		"/api/v1/../chat/completions",
		"/api/v1/%2e%2e/chat/completions",
		"/api/v1//chat/completions",
		"/api/v1/chat/completions?url=https://attacker.invalid",
		"https://attacker.invalid/api/v1/chat/completions",
		"/api/v1/models/other/model/endpoints",
	] {
		let r = f.request(&f.claims(), "POST", uri, chat()).await;
		assert_eq!(r.status(), 403, "{uri}");
	}
	let r = f
		.request(
			&f.claims(),
			"GET",
			"/api/v1/models/other/model/endpoints",
			Value::Null,
		)
		.await;
	assert_eq!(json_body(r).await["error"]["code"], "capability_model");
	let mut body = chat();
	body["test"] = json!("redirect");
	let r = f
		.request(&f.claims(), "POST", "/api/v1/chat/completions", body)
		.await;
	assert_eq!(r.status(), 302);
	assert!(r.headers().get(header::LOCATION).is_none());
	assert_eq!(f.provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn chat_discovery_and_embeddings_relay_and_cache_only_the_pinned_version() {
	let f = Fixture::new().await;
	let mut c = f.claims();
	for (method, path, body) in [
		("POST", "chat/completions", chat()),
		("GET", "models/author/model/endpoints", Value::Null),
		("GET", "endpoints/zdr", Value::Null),
		(
			"POST",
			"embeddings",
			json!({"model":"author/model","provider":{"zdr":true},"input":"text"}),
		),
	] {
		let r = f
			.request(&c, method, &format!("/api/v1/{path}"), body)
			.await;
		assert_eq!(r.status(), 200);
		json_body(r).await;
	}
	assert_eq!(f.provider.calls.load(Ordering::SeqCst), 4);
	assert_eq!(f.source.reads.load(Ordering::SeqCst), 1);
	c.version = "2".into();
	let r = f
		.request(&c, "POST", "/api/v1/chat/completions", chat())
		.await;
	json_body(r).await;
	assert_eq!(f.source.reads.load(Ordering::SeqCst), 2);
	assert!(
		f.source
			.secrets
			.lock()
			.unwrap()
			.iter()
			.all(|s| s == &format!("projects/1234/secrets/aidash-develop-cred-{}", c.credential))
	);
	assert!(
		f.provider
			.bearer
			.lock()
			.unwrap()
			.iter()
			.all(|b| b == &format!("Bearer {CANARY}"))
	);
	let logs = f.logs.0.lock().unwrap();
	assert_eq!(logs.len(), 5);
	assert!(
		logs.iter()
			.all(|l| !l.contains(CANARY) && !l.contains("untrusted"))
	);
}

#[tokio::test]
async fn stream_progress_arrives_before_completion_and_usage_is_audited() {
	let f = Fixture::new().await;
	let c = f.claims();
	let mut body = chat();
	body["stream"] = json!(true);
	let response = f
		.request(&c, "POST", "/api/v1/chat/completions", body)
		.await;
	assert_eq!(response.status(), 200);
	let mut stream = response.into_body().into_data_stream();
	let chunk = tokio::time::timeout(Duration::from_secs(1), stream.next())
		.await
		.unwrap()
		.unwrap()
		.unwrap();
	assert!(std::str::from_utf8(&chunk).unwrap().contains("early"));
	assert!(!f.provider.completed.load(Ordering::SeqCst));
	assert!(f.logs.0.lock().unwrap().is_empty());
	f.provider.finish.notify_one();
	while let Some(chunk) = stream.next().await {
		chunk.unwrap();
	}
	let logs = f.logs.0.lock().unwrap();
	assert_eq!(logs.len(), 1);
	let entry: Value = serde_json::from_str(&logs[0]).unwrap();
	assert_eq!(entry["jti"], c.jti.to_string());
	assert_eq!(entry["usage"]["total_tokens"], 6);
}

#[tokio::test]
async fn sanitized_errors_and_audits_omit_canary_and_disconnects_are_audited() {
	let f = Fixture::new().await;
	for mode in ["error", "audio"] {
		let mut body = chat();
		body["test"] = json!(mode);
		let r = f
			.request(&f.claims(), "POST", "/api/v1/chat/completions", body)
			.await;
		assert_eq!(r.status(), 400);
		let body = json_body(r).await.to_string();
		assert!(!body.contains(CANARY));
	}
	let mut body = chat();
	body["stream"] = json!(true);
	let r = f
		.request(&f.claims(), "POST", "/api/v1/chat/completions", body)
		.await;
	let mut stream = r.into_body().into_data_stream();
	stream.next().await.unwrap().unwrap();
	drop(stream);
	let logs = f.logs.0.lock().unwrap();
	assert_eq!(logs.len(), 3);
	assert!(
		logs.iter()
			.all(|l| !l.contains(CANARY) && !l.contains("Bearer") && !l.contains("choices"))
	);
	assert_eq!(
		serde_json::from_str::<Value>(&logs[2]).unwrap()["status"],
		499
	);
}

#[tokio::test]
async fn unavailable_version_and_body_limits_fail_closed() {
	let f = Fixture::new().await;
	f.source.unavailable.store(true, Ordering::SeqCst);
	let r = f
		.request(&f.claims(), "POST", "/api/v1/chat/completions", chat())
		.await;
	assert_eq!(r.status(), 403);
	assert_eq!(json_body(r).await["error"]["code"], "capability_credential");
	assert_eq!(f.provider.calls.load(Ordering::SeqCst), 0);
	f.source.unavailable.store(false, Ordering::SeqCst);
	let mut body = chat();
	body["input"] = json!("x".repeat(CHAT_REQUEST_LIMIT));
	let r = f
		.request(&f.claims(), "POST", "/api/v1/chat/completions", body)
		.await;
	assert_eq!(r.status(), 403);
	assert_eq!(
		json_body(r).await["error"]["code"],
		"capability_claim_violation"
	);
	let mut body = chat();
	body["input"] = json!("x".repeat(REQUEST_LIMIT));
	let r = f
		.request(&f.claims(), "POST", "/api/v1/embeddings", body)
		.await;
	assert_eq!(r.status(), 403);
	assert_eq!(
		json_body(r).await["error"]["code"],
		"capability_claim_violation"
	);
	let mut body = chat();
	body["test"] = json!("huge");
	let r = f
		.request(&f.claims(), "POST", "/api/v1/chat/completions", body)
		.await;
	assert!(to_bytes(r.into_body(), 2 * 1024 * 1024).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn cache_expiry_and_token_bucket_refill_are_bounded() {
	let signer = InMemorySigner::new("kms/1".into(), [7; 32]);
	let mut keys = PublicKeys::default();
	keys.insert(signer.kid().into(), signer.public_key());
	let source = Arc::new(Source::default());
	let mut cfg = config();
	cfg.burst = 3;
	cfg.requests_per_second = 1.0;
	let broker = Broker::new(cfg, keys, source.clone(), Arc::new(Logs::default())).unwrap();
	let c = Claims {
		iss: "worker".into(),
		aud: "environment".into(),
		iat: 1,
		exp: 61,
		jti: uuid::Uuid::new_v4(),
		kid: signer.kid().into(),
		tenant: "tenant-a".into(),
		provider: "openrouter".into(),
		credential: uuid::Uuid::from_u128(100),
		version: "1".into(),
		sub: TokenSubject::Run {
			run: "run".into(),
			call: uuid::Uuid::new_v4(),
		},
		ops: vec![Operation::Chat],
		model: "author/model".into(),
		max_output_tokens: 10,
	};
	broker.key(&c).await.unwrap();
	source.unavailable.store(true, Ordering::SeqCst);
	assert!(broker.key(&c).await.is_ok());
	assert_eq!(source.reads.load(Ordering::SeqCst), 1);
	assert!(broker.rate_limit(c.credential));
	assert!(broker.rate_limit(c.credential));
	assert!(broker.rate_limit(c.credential));
	assert!(!broker.rate_limit(c.credential));
	tokio::time::advance(Duration::from_secs(1)).await;
	assert!(broker.rate_limit(c.credential));
	tokio::time::advance(Duration::from_secs(59)).await;
	assert!(broker.key(&c).await.is_err());
	assert_eq!(source.reads.load(Ordering::SeqCst), 2);
}

#[test]
fn configuration_rejects_bursts_that_cannot_admit_one_media_flow() {
	for burst in 0..3 {
		let mut cfg = config();
		cfg.burst = burst;
		assert!(
			Broker::new(
				cfg,
				PublicKeys::default(),
				Arc::new(Source::default()),
				Arc::new(Logs::default()),
			)
			.is_err()
		);
	}
}
