use super::*;
use aidash_application::provider_access::Access;
use std::sync::{
	Mutex,
	atomic::{AtomicUsize, Ordering},
};

/// Stands in for the Tenant's Provider Credential: the fake OpenRouter base URL
/// and a fixed bearer, recording every call scope.
#[derive(Default)]
struct FixtureAccess {
	endpoint: String,
	scopes: Mutex<Vec<Context>>,
	sources: Mutex<Vec<String>>,
	unbound: bool,
	checks: AtomicUsize,
}
#[async_trait]
impl ProviderAccess for FixtureAccess {
	async fn resolve(
		&self,
		context: &Context,
		endpoint: &str,
		source: &Source,
	) -> aidash_application::Result<Access> {
		assert_eq!(endpoint, Provider::Openrouter.base_url());
		self.scopes.lock().unwrap().push(context.clone());
		self.sources.lock().unwrap().push(format!("{source:?}"));
		if self.unbound {
			return Err(Error::Invalid(
				"Provider Credential binding is missing".into(),
			));
		}
		Ok(Access {
			endpoint: self.endpoint.clone(),
			bearer: "fixture-key".to_owned().into(),
			keyless: None,
		})
	}
	async fn check(&self, _: &Context, _: &str, _: &Source) -> aidash_application::Result<()> {
		self.checks.fetch_add(1, Ordering::SeqCst);
		if self.unbound {
			return Err(Error::Invalid(
				"Provider Credential binding is missing".into(),
			));
		}
		Ok(())
	}
}
struct TestServer(tokio::task::JoinHandle<()>);
impl Drop for TestServer {
	fn drop(&mut self) {
		self.0.abort();
	}
}
fn run_context() -> Context {
	Context {
		tenant: "tenant-a".into(),
		run: Some(Uuid::now_v7()),
		// A Decision Attempt never uses the Run's admitted inference pin.
		provider_credential_id: Some(Uuid::now_v7()),
		..Default::default()
	}
}
fn client(access: Arc<FixtureAccess>) -> JevClient {
	JevClient::new(
		reqwest::Client::builder()
			.redirect(reqwest::redirect::Policy::none())
			.timeout(std::time::Duration::from_secs(5))
			.build()
			.unwrap(),
		"typesafe/jev-1.13".into(),
		access,
		run_context(),
	)
	.unwrap()
}
fn questions() -> Questions {
	json!({"q":{"type":"noul","instructions":"keep?"}})
		.as_object()
		.unwrap()
		.clone()
}

#[rstest::rstest]
fn construction_requires_an_exact_jev_release_and_one_run_in_a_tenant() {
	let access = Arc::new(FixtureAccess::default());
	for model in ["jev-latest", "~typesafe/jev-latest", "typesafe/jev-1.13.0"] {
		assert!(
			JevClient::new(
				reqwest::Client::new(),
				model.into(),
				access.clone(),
				run_context()
			)
			.is_err(),
			"{model}"
		);
	}
	let mut runless = run_context();
	runless.run = None;
	let mut maintenance = run_context();
	maintenance.maintenance =
		Some(aidash_application::provider_access::MaintenancePurpose::MemoryIndexing);
	let mut tenantless = run_context();
	tenantless.tenant.clear();
	for context in [runless, maintenance, tenantless] {
		assert!(
			JevClient::new(
				reqwest::Client::new(),
				"typesafe/jev-1.13".into(),
				access.clone(),
				context
			)
			.is_err()
		);
	}
}

#[rstest::rstest]
fn malformed_missing_and_out_of_range_probabilities_are_rejected() {
	for answer in [
		json!(null),
		json!({}),
		json!({"noul":"0.5"}),
		json!({"noul":-0.1}),
		json!({"noul":1.1}),
		json!({"type":"choice","noul":0.5}),
	] {
		assert!(probability(&json!({"answers":{"q":answer}}), "q").is_err());
	}
	assert!(probability(&json!({"answers":{}}), "q").is_err());
	for value in [0.0, 0.5, 1.0] {
		assert_eq!(
			probability(&json!({"answers":{"q":{"noul":value}}}), "q").unwrap(),
			value
		);
	}
}

#[rstest::rstest]
#[tokio::test]
async fn system_one_requests_carry_zdr_and_a_decision_scope_and_redact_failures() {
	use axum::{
		Json, Router,
		http::{HeaderMap, StatusCode},
		response::IntoResponse,
		routing::post,
	};
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let base = format!("http://{}/api/v1", listener.local_addr().unwrap());
	let app = Router::new().route(
		"/api/v1/systemone",
		post(|headers: HeaderMap, Json(body): Json<Value>| async move {
			assert_eq!(headers["authorization"], "Bearer fixture-key");
			assert_eq!(body["model"], "typesafe/jev-1.13");
			assert_eq!(body["provider"], json!({"zdr": true}));
			assert_eq!(body["questions"]["q"]["type"], "noul");
			let answered = json!({"answers":{"q":{"type":"noul","noul":0.75}},
				"usage":{"input_tokens":10,"output_tokens":2,"cost":0.0000004}});
			match body["state"].as_str().unwrap() {
				"http-error" => {
					(StatusCode::TOO_MANY_REQUESTS, "private provider detail").into_response()
				}
				"malformed" => (StatusCode::OK, "not json").into_response(),
				"missing" => Json(json!({"other":{}})).into_response(),
				"other-model" => {
					let mut value = answered;
					value["model"] = json!("typesafe/jev-1.14-20261001");
					Json(value).into_response()
				}
				_ => {
					let mut value = answered;
					value["model"] = json!("typesafe/jev-1.13-20260917");
					Json(value).into_response()
				}
			}
		}),
	);
	let _server = TestServer(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap()
	}));
	let access = Arc::new(FixtureAccess {
		endpoint: base,
		..Default::default()
	});
	let client = client(access.clone());
	let response = client.ask(&json!("ok"), &questions()).await.unwrap();
	assert_eq!(probability(&response, "q").unwrap(), 0.75);
	let scope = access.scopes.lock().unwrap()[0].clone();
	let inference = scope.inference.as_ref().unwrap();
	assert!(scope.is_decision());
	assert_eq!(inference.model, "typesafe/jev-1.13");
	assert_eq!(inference.max_output_tokens, 0);
	assert!(inference.call.is_some_and(|call| !call.is_nil()));
	assert_eq!(scope.provider_credential_id, None);
	assert_eq!(scope.tenant, "tenant-a");
	assert!(access.sources.lock().unwrap()[0].contains("openrouter"));
	for state in ["http-error", "malformed", "missing", "other-model"] {
		let error = client.ask(&json!(state), &questions()).await.unwrap_err();
		assert!(!error.to_string().contains("private provider detail"));
		assert!(!error.to_string().contains("fixture-key"));
	}
	// Each request is its own Decision Attempt.
	let calls: std::collections::BTreeSet<_> = access
		.scopes
		.lock()
		.unwrap()
		.iter()
		.map(|scope| scope.inference.as_ref().unwrap().call)
		.collect();
	assert_eq!(calls.len(), 5);
}

#[rstest::rstest]
#[tokio::test]
async fn a_missing_binding_fails_before_any_request_and_remotely_as_configuration() {
	let access = Arc::new(FixtureAccess {
		endpoint: "http://127.0.0.1:9/api/v1".into(),
		unbound: true,
		..Default::default()
	});
	let client = client(access.clone());
	assert!(client.check_credential().await.is_err());
	assert!(matches!(
		client.check_remote_credential().await.unwrap_err(),
		Error::RemoteSemantic(aidash_domain::semantic::Failure::Configuration)
	));
	assert_eq!(access.checks.load(Ordering::SeqCst), 2);
	assert!(
		client
			.ask(&json!({}), &questions())
			.await
			.unwrap_err()
			.to_string()
			.contains("binding is missing")
	);
	assert!(matches!(
		client
			.ask_remote(&json!({}), &questions())
			.await
			.unwrap_err(),
		Error::RemoteSemantic(aidash_domain::semantic::Failure::Configuration)
	));
}

#[rstest::rstest]
#[tokio::test]
async fn bounded_transport_rejects_oversize_requests_responses_and_redirects() {
	use axum::{Router, response::IntoResponse, routing::post};
	let requests = Arc::new(AtomicUsize::new(0));
	let seen = requests.clone();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let address = listener.local_addr().unwrap();
	let app = Router::new()
		.route(
			"/oversize/systemone",
			post(move || {
				let seen = seen.clone();
				async move {
					seen.fetch_add(1, Ordering::SeqCst);
					"x".repeat(129)
				}
			}),
		)
		.route(
			"/redirect/systemone",
			post(|| async {
				(
					axum::http::StatusCode::TEMPORARY_REDIRECT,
					[("location", "/oversize/systemone")],
				)
					.into_response()
			}),
		);
	let _server = TestServer(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap()
	}));
	let oversize = Arc::new(FixtureAccess {
		endpoint: format!("http://{address}/oversize"),
		..Default::default()
	});
	let transport = client(oversize).with_bounds(1024, 1, 128);
	assert!(
		transport
			.ask(&json!("x".repeat(1024)), &questions())
			.await
			.unwrap_err()
			.to_string()
			.contains("request exceeds")
	);
	let mut excess = questions();
	excess.insert("another".into(), json!({"type":"noul"}));
	assert!(transport.ask(&json!({}), &excess).await.is_err());
	assert_eq!(requests.load(Ordering::SeqCst), 0);
	assert!(
		transport
			.ask(&json!({}), &questions())
			.await
			.unwrap_err()
			.to_string()
			.contains("response exceeds")
	);
	assert_eq!(requests.load(Ordering::SeqCst), 1);
	let redirect = Arc::new(FixtureAccess {
		endpoint: format!("http://{address}/redirect"),
		..Default::default()
	});
	assert!(
		client(redirect)
			.with_bounds(1024, 1, 128)
			.ask(&json!({}), &questions())
			.await
			.unwrap_err()
			.to_string()
			.contains("307")
	);
	assert_eq!(
		requests.load(Ordering::SeqCst),
		1,
		"unapproved redirect target must not receive history"
	);
}
