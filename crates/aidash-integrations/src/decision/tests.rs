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
	checks: AtomicUsize,
	resolutions: AtomicUsize,
	scopes: Mutex<Vec<Context>>,
}
impl AccessFixture {
	fn new(endpoint: &str) -> Arc<Self> {
		Arc::new(Self {
			endpoint: endpoint.into(),
			available: AtomicBool::new(true),
			checks: AtomicUsize::new(0),
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
			keyless: None,
		})
	}
	async fn check(&self, context: &Context, endpoint: &str, source: &Source) -> Result<()> {
		assert_eq!(endpoint, Provider::Openrouter.base_url());
		assert!(matches!(source, Source::Tenant { provider } if provider == "openrouter"));
		assert!(context.is_decision());
		self.checks.fetch_add(1, Ordering::SeqCst);
		if !self.available.load(Ordering::SeqCst) {
			return Err(Error::Invalid("PRIVATE_CREDENTIAL_BACKEND_ERROR".into()));
		}
		Ok(())
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
impl RouteCapacity for CapacityFixture {
	fn fits(
		&self,
		route: &DeciderRoute,
		_: &Value,
		questions: &Questions,
		_: &[u8],
	) -> Result<bool> {
		assert_eq!(route.model, "typesafe/jev-1.13");
		Ok(self.0.is_none_or(|max| questions.len() <= max))
	}
}
fn jev() -> DeciderRoute {
	DeciderRoute::jev("typesafe/jev-1.13")
}
fn config() -> DeciderConfig {
	DeciderConfig {
		hook: Hook::Compaction,
		answer_type: AnswerType::Noul,
		description: "Keep context".into(),
		provider_credential: "openrouter".into(),
		routes: vec![DeciderRoute::jev("typesafe/jev-1.13")],
		builder: BUILDER.into(),
		option_source: OPTION_SOURCE.into(),
		rule: RULE.into(),
		mode: Mode::Enforce,
		max_calls_per_run: 20,
		state_retention: Default::default(),
	}
}
fn provider(endpoint: &str, cap: Option<usize>) -> RouteDecisionProvider {
	RouteDecisionProvider::new(
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
#[tokio::test]
async fn request_planning_splits_only_under_provider_capacity_and_preserves_all_candidates() {
	let client = provider("https://example.test", Some(2));
	let state = json!({"goal":"complete"});
	let questions = questions(7);
	let plan = client.plan(0, &state, &questions).unwrap();
	assert_eq!(plan.len(), 4);
	let mut coverage = BTreeMap::new();
	for batch in &plan {
		client.prepare(0, batch, Uuid::now_v7()).await.unwrap();
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
			.plan(0, &state, &questions)
			.is_err()
	);
}
#[tokio::test]
async fn requests_exceed_legacy_size_and_question_caps_without_truncation() {
	let client = provider("https://example.test", None);
	let state = json!({"goal":"x".repeat(1_100_000)});
	let questions = questions(1100);
	let plan = client.plan(0, &state, &questions).unwrap();
	assert_eq!(plan.len(), 1);
	assert!(plan[0].body.len() > 1_048_576);
	assert_eq!(plan[0].questions.len(), 1100);
	client.prepare(0, &plan[0], Uuid::now_v7()).await.unwrap();
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
		validate_response(&jev(), &questions, &serde_json::to_vec(&good).unwrap())
			.unwrap()
			.answers["q0"],
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
			validate_response(&jev(), &questions, &serde_json::to_vec(&wrong).unwrap()).is_err()
		);
	}
	let exact = f64::from_bits(0x3fdfffffffffffff);
	let wire = format!(
		"{{\"model\":\"typesafe/jev-1.13\",\"answers\":{{\"q0\":{{\"noul\":{exact:?}}}}}}}"
	);

	assert_eq!(
		validate_response(&jev(), &questions, wire.as_bytes())
			.unwrap()
			.answers["q0"]
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
		assert!(validate_response(&jev(), &questions, malformed).is_err());
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
async fn each_attempt_checks_before_reservation_and_resolves_its_credential_only_to_dispatch() {
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
	let client = RouteDecisionProvider::new(
		config(),
		"node-decision-adapter:23".into(),
		access.clone(),
		run_context(),
		Arc::new(CapacityFixture(Some(1))),
		Duration::from_secs(5),
	)
	.unwrap();
	let requests = client
		.plan(0, &json!({"goal":"authorized"}), &questions(3))
		.unwrap();
	let attempts = [Uuid::now_v7(), Uuid::now_v7()];
	let mut transports = vec![];
	for (request, attempt) in requests.iter().zip(attempts) {
		transports.push(client.prepare(0, request, attempt).await.unwrap());
	}
	// Preparing checks the binding without minting a short-lived call credential,
	// which a reservation wait could outlive.
	assert_eq!(access.checks.load(Ordering::SeqCst), 2);
	assert_eq!(access.resolutions.load(Ordering::SeqCst), 0);
	assert!(received.lock().unwrap().is_empty());
	for (transport, request) in transports.into_iter().zip(&requests) {
		let answers = transport.dispatch().await.unwrap();
		assert_eq!(answers.len(), request.questions.len());
		assert!(answers.values().all(|p| *p == Probability::half()));
	}
	// Each dispatch resolves the credential once, as its Decision Attempt.
	assert_eq!(access.resolutions.load(Ordering::SeqCst), 2);
	for (scope, attempt) in access.scopes.lock().unwrap().iter().zip(attempts) {
		assert!(scope.is_decision());
		let inference = scope.inference.as_ref().unwrap();
		assert_eq!(inference.max_output_tokens, 0);
		assert_eq!(inference.call, Some(attempt));
	}
	// Revocation after preparation fails the dispatch before any request.
	let late = client
		.prepare(0, &requests[2], Uuid::now_v7())
		.await
		.unwrap();
	access.available.store(false, Ordering::SeqCst);
	let error = late.dispatch().await.unwrap_err();
	assert!(matches!(error, DispatchError::ProviderFailure(_)));
	assert!(
		!error
			.to_string()
			.contains("PRIVATE_CREDENTIAL_BACKEND_ERROR")
	);
	// A revoked or unbound credential fails preparation, so no attempt is
	// reserved and no request is sent.
	let error = client
		.prepare(0, &requests[2], Uuid::now_v7())
		.await
		.err()
		.unwrap()
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
		.plan(0, &json!({"goal":"authorized"}), &questions(1))
		.unwrap()
		.remove(0);
	assert_eq!(
		client
			.prepare(0, &request, Uuid::now_v7())
			.await
			.unwrap()
			.dispatch()
			.await
			.unwrap()["q0"],
		Probability::half()
	);
	let router = Router::new().route(
		"/systemone",
		post(|| async { (StatusCode::BAD_REQUEST, "PRIVATE_HISTORY_AND_SECRET") }),
	);
	let (endpoint, _server) = server(router).await;
	let client = provider(&endpoint, None);
	let error = client
		.prepare(0, &request, Uuid::now_v7())
		.await
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
	let request = client.plan(0, &json!({}), &questions(1)).unwrap().remove(0);
	assert!(
		client
			.prepare(0, &request, Uuid::now_v7())
			.await
			.unwrap()
			.dispatch()
			.await
			.is_err()
	);
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
		let request = client.plan(0, &json!({}), &questions(1)).unwrap().remove(0);
		assert!(matches!(
			client
				.prepare(0, &request, Uuid::now_v7())
				.await
				.unwrap()
				.dispatch()
				.await,
			Err(DispatchError::InvalidAnswers)
		));
	}
}
#[tokio::test]
async fn prepared_body_cannot_change_the_model_or_question_map() {
	let client = provider("https://example.test", None);
	let mut request = client.plan(0, &json!({}), &questions(1)).unwrap().remove(0);
	let mut body: Value = serde_json::from_slice(&request.body).unwrap();
	body["model"] = "jev-latest".into();
	request.body = serde_json::to_vec(&body).unwrap();
	assert!(client.prepare(0, &request, Uuid::now_v7()).await.is_err());
}
#[test]
fn answered_responses_report_the_served_model_and_numeric_usage_for_the_attempt_trace() {
	let questions = questions(1);
	let answers = json!({"q0":{"type":"noul","noul":0.5}});
	let answered = validate_response(
		&jev(),
		&questions,
		&serde_json::to_vec(&json!({
			"model":"typesafe/jev-1.13-20260917","answers":answers,
			"usage":{"input_tokens":12,"cost":0.000002}
		}))
		.unwrap(),
	)
	.unwrap();
	assert_eq!(
		(
			answered.served.as_str(),
			answered.input_tokens,
			answered.cost_usd
		),
		("typesafe/jev-1.13-20260917", Some(12), Some(0.000002))
	);
	// Malformed or missing usage never rejects otherwise valid answers.
	for usage in [
		json!(null),
		json!("free"),
		json!({"input_tokens":"12","cost":[]}),
	] {
		let answered = validate_response(
			&jev(),
			&questions,
			&serde_json::to_vec(
				&json!({"model":"typesafe/jev-1.13","answers":answers,"usage":usage}),
			)
			.unwrap(),
		)
		.unwrap();
		assert_eq!((answered.input_tokens, answered.cost_usd), (None, None));
	}
}

#[test]
fn the_byte_budget_keeps_requests_in_the_mode_their_state_allows() {
	let route = jev();
	let capacity = ByteBudget;
	let questions = Questions::new();
	let state = |bytes: usize| json!("s".repeat(bytes - 2));
	let small = state(1_000);
	assert!(
		capacity
			.fits(&route, &small, &questions, &[0; 32_000])
			.unwrap()
	);
	assert!(
		!capacity
			.fits(&route, &small, &questions, &[0; 32_001])
			.unwrap()
	);
	// A state beyond the default mode travels in the long-context mode.
	let large = state(30_000);
	assert!(
		capacity
			.fits(&route, &large, &questions, &[0; 40_000])
			.unwrap()
	);
	let too_large = state(560_001);
	assert!(
		!capacity
			.fits(&route, &too_large, &questions, &[0; 1])
			.unwrap()
	);
}

fn clef() -> DeciderRoute {
	DeciderRoute {
		provider_contract: DECISIONS.into(),
		model: "cloudflare/clef".into(),
		hosts: vec!["cloudflare".into(), "deepinfra".into()],
		threshold: Probability::half(),
		state_budget: StateBudget {
			long_context_request_bytes: None,
			long_context_state_bytes: None,
			..StateBudget::default()
		},
	}
}
/// A provider whose second Route is Clef, served under an OpenRouter-shaped base.
fn routed(endpoint: &str) -> RouteDecisionProvider {
	RouteDecisionProvider::new(
		DeciderConfig {
			routes: vec![jev(), clef()],
			..config()
		},
		"node-decision-adapter:23".into(),
		AccessFixture::new(&format!("{endpoint}/api/v1")),
		run_context(),
		Arc::new(ByteBudget),
		Duration::from_secs(5),
	)
	.unwrap()
}
#[tokio::test]
async fn a_decisions_route_names_its_hosts_and_is_answered_only_by_them() {
	let host = Arc::new(Mutex::new("DeepInfra"));
	let served = host.clone();
	let router = Router::new().route(
		"/api/alpha/decisions",
		post(move |bytes: axum::body::Bytes| {
			let host = served.clone();
			async move {
				let body: Value = serde_json::from_slice(&bytes).unwrap();
				assert_eq!(body["model"], "cloudflare/clef");
				// Only the hosts the Route approved, with zero data retention.
				assert_eq!(
					body["provider"],
					json!({"zdr":true,"only":["cloudflare","deepinfra"],"allow_fallbacks":true})
				);
				Json(
					json!({"model":"cloudflare/clef","provider":*host.lock().unwrap(),
					"answers":{"q0":{"type":"noul","noul":0.25}}}),
				)
			}
		}),
	);
	let (endpoint, _server) = server(router).await;
	let client = routed(&endpoint);
	let request = client
		.plan(1, &json!({"goal":"authorized"}), &questions(1))
		.unwrap()
		.remove(0);
	// The Route's request differs from the System One one for the same state.
	assert_ne!(
		request.body,
		client
			.plan(0, &json!({"goal":"authorized"}), &questions(1))
			.unwrap()[0]
			.body
	);
	let dispatch = || async {
		client
			.prepare(1, &request, Uuid::now_v7())
			.await
			.unwrap()
			.dispatch()
			.await
	};
	assert_eq!(dispatch().await.unwrap()["q0"].value(), 0.25);
	*host.lock().unwrap() = "Cloudflare";
	assert!(dispatch().await.is_ok());
	// A host the Route does not name is an invalid answer, never accepted.
	*host.lock().unwrap() = "PrimeIntellect";
	assert!(matches!(
		dispatch().await,
		Err(DispatchError::InvalidAnswers)
	));
	assert!(client.plan(2, &json!({}), &questions(1)).is_err());
}
#[tokio::test]
async fn refusals_are_told_apart_from_a_route_being_unavailable() {
	let status = Arc::new(AtomicUsize::new(402));
	let returned = status.clone();
	let router = Router::new().route(
		"/api/alpha/decisions",
		post(move || {
			let status = returned.clone();
			async move { StatusCode::from_u16(status.load(Ordering::SeqCst) as u16).unwrap() }
		}),
	);
	let (endpoint, _server) = server(router).await;
	let client = routed(&endpoint);
	let request = client.plan(1, &json!({}), &questions(1)).unwrap().remove(0);
	for (code, refused) in [
		(400, true),
		(401, true),
		(402, true),
		(403, true),
		(413, true),
		(404, false),
		(429, false),
		(500, false),
		(503, false),
		(529, false),
	] {
		status.store(code, Ordering::SeqCst);
		let result = client
			.prepare(1, &request, Uuid::now_v7())
			.await
			.unwrap()
			.dispatch()
			.await;
		match result {
			Err(DispatchError::Refused(returned)) => {
				assert!(refused, "{code}");
				assert_eq!(usize::from(returned), code);
			}
			Err(DispatchError::ProviderFailure(_)) => assert!(!refused, "{code}"),
			other => panic!("{code}: {other:?}"),
		}
	}
}
#[test]
fn the_decisions_api_is_the_alpha_endpoint_beside_the_v1_base() {
	assert_eq!(
		route_url("https://openrouter.ai/api/v1/", DECISIONS).unwrap(),
		"https://openrouter.ai/api/alpha/decisions"
	);
	assert_eq!(
		route_url("https://broker.example/api/v1", PROVIDER).unwrap(),
		"https://broker.example/api/v1/systemone"
	);
	assert!(route_url("https://openrouter.ai/api", DECISIONS).is_err());
	assert!(named_host(&["primeintellect".into()], "PrimeIntellect"));
	assert!(!named_host(&["deepinfra".into()], ""));
	assert!(!named_host(&["deepinfra".into()], "Cloudflare"));
}

/// The built-in approval Decider's requests, from the state a pending call
/// builds to the verdict its Route's answers give.
#[tokio::test]
async fn an_approval_request_asks_the_block_questions_of_the_pending_call_on_each_route() {
	use aidash_application::{
		decision::{ApprovalTool, build_approval_state},
		registry::system::approval_decider,
	};
	use aidash_domain::{provider::ToolCall, run_input::RunInput};

	let decider: DeciderConfig =
		serde_json::from_value(approval_decider().unwrap().config).unwrap();
	let questions = approval_questions();
	let state = build_approval_state(
		&aidash_domain::context::Context::default(),
		&[RunInput {
			seq: 1,
			sender: "person".into(),
			content: "Post the Q3 summary to the shared endpoint".into(),
			idempotency_key: "key".into(),
			message_id: None,
			reference_only: false,
		}],
		"You are the reporting agent.",
		&ToolCall {
			id: "pending".into(),
			name: "http_post".into(),
			arguments: json!({"body":"Q3 summary"}),
		},
		&ApprovalTool {
			id: "acme.http_post".into(),
			version: "1.0.0".into(),
			description: "Post a document".into(),
			input_schema: json!({"type":"object"}),
			target: Some("https://api.example.com".into()),
		},
		&decider.routes[0].state_budget,
	)
	.unwrap();
	assert!(state.fits);

	let bodies = Arc::new(Mutex::new(Vec::<Value>::new()));
	let answering = |model: &'static str, host: Option<&'static str>| {
		let bodies = bodies.clone();
		post(move |bytes: axum::body::Bytes| {
			let bodies = bodies.clone();
			async move {
				bodies
					.lock()
					.unwrap()
					.push(serde_json::from_slice(&bytes).unwrap());
				let answers: BTreeMap<_, _> = BlockReason::CATEGORIES
					.into_iter()
					.map(|reason| {
						let noul = if reason == BlockReason::Injected {
							0.9
						} else {
							0.01
						};
						(reason.id(), json!({"type":"noul","noul":noul}))
					})
					.collect();
				Json(json!({"model":model,"provider":host,"answers":answers}))
			}
		})
	};
	let router = Router::new()
		.route("/api/v1/systemone", answering("typesafe/jev-1.14", None))
		.route(
			"/api/alpha/decisions",
			answering("cloudflare/clef", Some("Cloudflare")),
		);
	let (endpoint, _server) = server(router).await;
	let client = RouteDecisionProvider::new(
		decider.clone(),
		"node-decision-adapter:23".into(),
		AccessFixture::new(&format!("{endpoint}/api/v1")),
		run_context(),
		Arc::new(ByteBudget),
		Duration::from_secs(5),
	)
	.unwrap();

	for (index, route) in decider.routes.iter().enumerate() {
		let requests = client.plan(index, &state.state, &questions).unwrap();
		assert_eq!(requests.len(), 1, "{}", route.model);
		let request = &requests[0];
		assert_eq!(request.questions, questions);
		let body: Value = serde_json::from_slice(&request.body).unwrap();
		assert_eq!(body["model"], route.model.as_str());
		assert_eq!(body["state"], state.state);
		let asked: BTreeMap<_, _> = body["questions"]
			.as_object()
			.unwrap()
			.iter()
			.map(|(id, question)| (id.clone(), question.clone()))
			.collect();
		assert_eq!(
			asked.keys().map(String::as_str).collect::<Vec<_>>(),
			[
				"beyond_request",
				"boundary",
				"data_exfiltration",
				"destructive",
				"injected"
			]
		);
		for (id, question) in &questions {
			assert_eq!(
				asked[id],
				json!({"type":"noul","instructions":question.description})
			);
		}
		let answers = client
			.prepare(index, request, Uuid::now_v7())
			.await
			.unwrap()
			.dispatch()
			.await
			.unwrap();
		assert_eq!(
			verdict(&answers, route.threshold).unwrap(),
			Verdict::Blocked(BlockReason::Injected)
		);
	}
	// Each Route's endpoint received exactly one request.
	assert_eq!(bodies.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn oversized_valid_decision_responses_are_refused_with_or_without_content_length() {
	for chunked in [false, true] {
		let router = Router::new().route("/systemone", post(move || async move {
   let body = serde_json::to_vec(&json!({"model":"typesafe/jev-1.13","answers":{"q0":{"noul":0.5}},"debug":"x".repeat(8 * 1024 * 1024)})).unwrap();
   if chunked {
    let chunks: Vec<_> = body.chunks(1024 * 1024).map(|chunk| Ok::<_, std::io::Error>(axum::body::Bytes::copy_from_slice(chunk))).collect();
    axum::response::Response::new(axum::body::Body::from_stream(futures_util::stream::iter(chunks)))
   } else {
    let size = body.len();
    let mut response = axum::response::Response::new(axum::body::Body::from(body));
    response.headers_mut().insert("content-length", size.to_string().parse().unwrap());
    response
   }
  }));
		let (endpoint, _server) = server(router).await;
		let client = provider(&endpoint, None);
		let request = client
			.plan(0, &json!({"goal":"authorized"}), &questions(1))
			.unwrap()
			.remove(0);
		let error = client
			.prepare(0, &request, Uuid::now_v7())
			.await
			.unwrap()
			.dispatch()
			.await
			.unwrap_err();
		assert!(matches!(error, DispatchError::InvalidAnswers), "{error}");
	}
}
