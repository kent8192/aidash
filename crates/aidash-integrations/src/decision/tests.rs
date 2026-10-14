use super::*;
use axum::{
	Json, Router,
	http::{HeaderMap, StatusCode},
	routing::post,
};
use std::sync::{
	Mutex,
	atomic::{AtomicBool, AtomicUsize, Ordering},
};

/// The Tenant's OpenRouter credential: a fake base URL and a fixed bearer.
struct AccessFixture {
	endpoint: String,
	available: AtomicBool,
	resolutions: AtomicUsize,
	scopes: Mutex<Vec<Context>>,
}
impl AccessFixture {
	fn new(endpoint: &str) -> Arc<Self> {
		Arc::new(Self {
			endpoint: endpoint.into(),
			available: AtomicBool::new(true),
			resolutions: AtomicUsize::new(0),
			scopes: Mutex::new(vec![]),
		})
	}
}
#[async_trait]
impl ProviderAccess for AccessFixture {
	async fn resolve(
		&self,
		context: &Context,
		endpoint: &str,
		source: &Source,
	) -> Result<aidash_application::provider_access::Access> {
		assert_eq!(endpoint, Provider::Openrouter.base_url());
		assert!(matches!(source, Source::Tenant { provider } if provider == "openrouter"));
		self.resolutions.fetch_add(1, Ordering::SeqCst);
		self.scopes.lock().unwrap().push(context.clone());
		if !self.available.load(Ordering::SeqCst) {
			return Err(Error::Invalid("PRIVATE_CREDENTIAL_BACKEND_ERROR".into()));
		}
		Ok(aidash_application::provider_access::Access {
			endpoint: self.endpoint.clone(),
			bearer: "fixture-secret".to_owned().into(),
		})
	}
}
fn run_context() -> Context {
	Context {
		tenant: "tenant-a".into(),
		run: Some(uuid::Uuid::now_v7()),
		..Default::default()
	}
}
struct CapacityFixture(Option<usize>);
impl JevCapacity for CapacityFixture {
	fn fits(&self, model: &str, _: &Value, questions: &Questions, _: &[u8]) -> Result<bool> {
		assert_eq!(model, "typesafe/jev-1.13");
		Ok(self.0.is_none_or(|max| questions.len() <= max))
	}
}
fn config() -> DeciderConfig {
	DeciderConfig {
		hook: Hook::Compaction,
		answer_type: AnswerType::Noul,
		description: "Keep context".into(),
		provider_contract: PROVIDER.into(),
		provider_credential: "openrouter".into(),
		model: "typesafe/jev-1.13".into(),
		builder: BUILDER.into(),
		option_source: OPTION_SOURCE.into(),
		rule: RULE.into(),
		keep_threshold: Probability::half(),
		mode: Mode::Enforce,
	}
}
fn provider(endpoint: &str, cap: Option<usize>) -> JevDecisionProvider {
	JevDecisionProvider::new(
		config(),
		"node-decision-adapter:23".into(),
		AccessFixture::new(endpoint),
		run_context(),
		Arc::new(CapacityFixture(cap)),
		Duration::from_secs(5),
	)
	.unwrap()
}
fn questions(count: usize) -> Questions {
	(0..count)
		.map(|i| {
			(
				format!("q{i}"),
				Question {
					description: "Does this exact call remain required?".into(),
					answer_type: AnswerType::Noul,
				},
			)
		})
		.collect()
}
#[test]
fn request_planning_splits_only_under_provider_capacity_and_preserves_all_candidates() {
	let client = provider("https://example.test", Some(2));
	let state = json!({"goal":"complete"});
	let questions = questions(7);
	let plan = client.plan(&state, &questions).unwrap();
	assert_eq!(plan.len(), 4);
	let mut coverage = BTreeMap::new();
	for batch in &plan {
		client.prepare(batch).unwrap();
		let payload: Value = serde_json::from_slice(&batch.body).unwrap();
		assert_eq!(payload["state"], state);
		assert_eq!(payload["model"], "typesafe/jev-1.13");
		for (id, q) in &batch.questions {
			assert!(coverage.insert(id.clone(), q.clone()).is_none());
		}
	}
	assert_eq!(coverage, questions);
	assert!(
		provider("https://example.test", Some(0))
			.plan(&state, &questions)
			.is_err()
	);
}
#[test]
fn requests_exceed_legacy_size_and_question_caps_without_truncation() {
	let client = provider("https://example.test", None);
	let state = json!({"goal":"x".repeat(1_100_000)});
	let questions = questions(1100);
	let plan = client.plan(&state, &questions).unwrap();
	assert_eq!(plan.len(), 1);
	assert!(plan[0].body.len() > 1_048_576);
	assert_eq!(plan[0].questions.len(), 1100);
	client.prepare(&plan[0]).unwrap();
	let payload: Value = serde_json::from_slice(&plan[0].body).unwrap();
	assert_eq!(payload["state"], state);
	assert!(
		!String::from_utf8(plan[0].body.clone())
			.unwrap()
			.contains("fixture-secret")
	);
}
#[test]
fn responses_require_exact_models_question_coverage_types_and_probabilities() {
	let questions = questions(1);
	let good = json!({"model":"typesafe/jev-1.13","answers":{"q0":{"type":"noul","noul":0.5}}});
	assert_eq!(
		validate_response(
			"typesafe/jev-1.13",
			&questions,
			&serde_json::to_vec(&good).unwrap()
		)
		.unwrap()["q0"],
		Probability::half()
	);
	for wrong in [
		json!({"answers":good["answers"]}),
		json!({"model":"typesafe/jev-1.14","answers":good["answers"]}),
		json!({"model":"jev-latest","answers":good["answers"]}),
		json!({"model":"typesafe/jev-1.13","answers":{}}),
		json!({"model":"typesafe/jev-1.13","answers":{"q0":{"noul":0.5},"extra":{"noul":0.5}}}),
		json!({"model":"typesafe/jev-1.13","answers":{"q0":{"type":"choice","noul":0.5}}}),
		json!({"model":"typesafe/jev-1.13","answers":{"q0":{"noul":-0.1}}}),
		json!({"model":"typesafe/jev-1.13","answers":{"q0":{"noul":1.01}}}),
	] {
		assert!(
			validate_response(
				"typesafe/jev-1.13",
				&questions,
				&serde_json::to_vec(&wrong).unwrap()
			)
			.is_err()
		);
	}
	let exact = f64::from_bits(0x3fdfffffffffffff);
	let wire = format!(
		"{{\"model\":\"typesafe/jev-1.13\",\"answers\":{{\"q0\":{{\"noul\":{exact:?}}}}}}}"
	);

	assert_eq!(
		validate_response("typesafe/jev-1.13", &questions, wire.as_bytes()).unwrap()["q0"]
			.value()
			.to_bits(),
		exact.to_bits()
	);
	for malformed in [
		br#"{"model":"typesafe/jev-1.13","answers":{"q0":{"noul":0.0},"q0":{"noul":1.0}}}"#
			.as_slice(),
		br#"{"model":"typesafe/jev-1.13","answers":{"q0":{"noul":1e999}}}"#.as_slice(),
		br#"{"model":"typesafe/jev-1.13","answers":{"q0":{"noul":"0.5"}}}"#.as_slice(),
	] {
		assert!(validate_response("typesafe/jev-1.13", &questions, malformed).is_err());
	}
}
struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
	fn drop(&mut self) {
		self.0.abort();
	}
}
async fn server(router: Router) -> (String, Server) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	// The catalog or broker base URL; dispatch appends `/systemone`.
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let handle = tokio::spawn(async move {
		axum::serve(listener, router).await.unwrap();
	});
	(endpoint, Server(handle))
}

#[tokio::test]
async fn each_dispatch_resolves_the_tenant_credential_and_fails_closed_after_revocation() {
	let received = Arc::new(Mutex::new(Vec::new()));
	let collected = received.clone();
	let router = Router::new().route(
		"/systemone",
		post(move |headers: HeaderMap, bytes: axum::body::Bytes| {
			let received = collected.clone();
			async move {
				assert_eq!(headers["authorization"], "Bearer fixture-secret");
				let body: Value = serde_json::from_slice(&bytes).unwrap();
				assert_eq!(body["model"], "typesafe/jev-1.13");
				assert_eq!(body["provider"], json!({"zdr": true}));
				received.lock().unwrap().push(bytes.to_vec());
				let answers: BTreeMap<_, _> = body["questions"]
					.as_object()
					.unwrap()
					.keys()
					.map(|id| (id.clone(), json!({"noul":0.5})))
					.collect();
				// OpenRouter names the dated snapshot that served the pinned release.
				Json(json!({"model":"typesafe/jev-1.13-20260917", "answers":answers}))
			}
		}),
	);
	let (endpoint, _server) = server(router).await;
	let access = AccessFixture::new(&endpoint);
	let client = JevDecisionProvider::new(
		config(),
		"node-decision-adapter:23".into(),
		access.clone(),
		run_context(),
		Arc::new(CapacityFixture(Some(1))),
		Duration::from_secs(5),
	)
	.unwrap();
	let requests = client
		.plan(&json!({"goal":"authorized"}), &questions(3))
		.unwrap();
	let transports: Vec<_> = requests
		.iter()
		.map(|r| client.prepare(r).unwrap())
		.collect();
	// Preparing never resolves a credential; each physical attempt does.
	assert_eq!(access.resolutions.load(Ordering::SeqCst), 0);
	let mut transports = transports.into_iter();
	for (transport, request) in transports.by_ref().take(2).zip(&requests) {
		let answers = transport.dispatch().await.unwrap();
		assert_eq!(answers.len(), request.questions.len());
		assert!(answers.values().all(|p| *p == Probability::half()));
	}
	assert_eq!(access.resolutions.load(Ordering::SeqCst), 2);
	for scope in access.scopes.lock().unwrap().iter() {
		assert!(scope.is_decision());
		assert_eq!(scope.inference.as_ref().unwrap().max_output_tokens, 0);
	}
	// A revoked or unbound credential stops the remaining attempt before any request.
	access.available.store(false, Ordering::SeqCst);
	let error = transports
		.next()
		.unwrap()
		.dispatch()
		.await
		.unwrap_err()
		.to_string();
	assert!(!error.contains("fixture-secret"));
	assert_eq!(
		*received.lock().unwrap(),
		requests[..2]
			.iter()
			.map(|r| r.body.clone())
			.collect::<Vec<_>>()
	);
}

#[tokio::test]
async fn exact_http_bytes_auth_large_response_and_safe_provider_errors() {
	let router = Router::new().route(
		"/systemone",
		post(|headers: HeaderMap, Json(body): Json<Value>| async move {
			assert_eq!(headers["authorization"], "Bearer fixture-secret");
			assert_eq!(body["model"], "typesafe/jev-1.13");
			assert_eq!(body["questions"]["q0"]["type"], "noul");
			Json(
				json!({"model":"typesafe/jev-1.13","answers":{"q0":{"noul":0.5}},"debug":"x".repeat(1_100_000)}),
			)
		}),
	);
	let (endpoint, _server) = server(router).await;
	let client = provider(&endpoint, None);
	let request = client
		.plan(&json!({"goal":"authorized"}), &questions(1))
		.unwrap()
		.remove(0);
	assert_eq!(
		client.prepare(&request).unwrap().dispatch().await.unwrap()["q0"],
		Probability::half()
	);
	let router = Router::new().route(
		"/systemone",
		post(|| async { (StatusCode::BAD_REQUEST, "PRIVATE_HISTORY_AND_SECRET") }),
	);
	let (endpoint, _server) = server(router).await;
	let client = provider(&endpoint, None);
	let error = client
		.prepare(&request)
		.unwrap()
		.dispatch()
		.await
		.unwrap_err()
		.to_string();
	assert!(!error.contains("PRIVATE_HISTORY_AND_SECRET"));
	assert!(error.contains("400"));
}
#[tokio::test]
async fn redirects_and_failures_do_not_create_unreserved_requests() {
	let calls = Arc::new(AtomicUsize::new(0));
	let counted = calls.clone();
	let target_calls = calls.clone();
	let router = Router::new()
		.route(
			"/systemone",
			post(move || {
				let calls = counted.clone();
				async move {
					calls.fetch_add(1, Ordering::SeqCst);
					(
						StatusCode::TEMPORARY_REDIRECT,
						[("location", "/target")],
						"redirect",
					)
				}
			}),
		)
		.route(
			"/target",
			post(move || {
				let calls = target_calls.clone();
				async move {
					calls.fetch_add(1, Ordering::SeqCst);
					StatusCode::OK
				}
			}),
		);
	let (endpoint, _server) = server(router).await;
	let client = provider(&endpoint, None);
	let request = client.plan(&json!({}), &questions(1)).unwrap().remove(0);
	assert!(client.prepare(&request).unwrap().dispatch().await.is_err());
	assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn successful_http_with_invalid_answers_is_a_contract_violation() {
	for body in [
		json!({"model":"typesafe/jev-1.13","answers":{}}),
		json!({"model":"typesafe/jev-1.14","answers":{"q0":{"noul":0.5}}}),
	] {
		let router = Router::new().route(
			"/systemone",
			post(move || {
				let body = body.clone();
				async move { Json(body) }
			}),
		);
		let (endpoint, _server) = server(router).await;
		let client = provider(&endpoint, None);
		let request = client.plan(&json!({}), &questions(1)).unwrap().remove(0);
		assert!(matches!(
			client.prepare(&request).unwrap().dispatch().await,
			Err(DispatchError::InvalidAnswers)
		));
	}
}
#[test]
fn prepared_body_cannot_change_the_model_or_question_map() {
	let client = provider("https://example.test", None);
	let mut request = client.plan(&json!({}), &questions(1)).unwrap().remove(0);
	let mut body: Value = serde_json::from_slice(&request.body).unwrap();
	body["model"] = "jev-latest".into();
	request.body = serde_json::to_vec(&body).unwrap();
	assert!(client.prepare(&request).is_err());
}
