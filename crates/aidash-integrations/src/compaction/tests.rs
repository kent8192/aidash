use super::*;
struct FixtureCredentials;
impl Credentials for FixtureCredentials {
	fn resolve(&self, _: &str) -> Result<String> {
		Ok("fixture-key".into())
	}
}
struct TestServer(tokio::task::JoinHandle<()>);
impl Drop for TestServer {
	fn drop(&mut self) {
		self.0.abort();
	}
}

#[rstest::rstest]
fn system_one_request_has_bearer_auth_and_probability_questions() {
	let client = JevClient::new(
		reqwest::Client::new(),
		"https://api.typesafe.ai/v1/systemone".into(),
		"jev-latest".into(),
		"AIDASH_SECRET_JEV".into(),
		Arc::new(FixtureCredentials),
	)
	.unwrap();
	let state = json!({"goal":"finish the task"});
	let questions = json!({"call_t1":{"type":"noul","instructions":"Keep this call?"}})
		.as_object()
		.unwrap()
		.clone();
	let request = client
		.request(&state, &questions, "fixture-key")
		.build()
		.unwrap();
	assert_eq!(
		request.url().as_str(),
		"https://api.typesafe.ai/v1/systemone"
	);
	assert_eq!(request.method(), reqwest::Method::POST);
	assert_eq!(request.headers()["authorization"], "Bearer fixture-key");
	let body: Value = serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
	assert_eq!(
		body,
		json!({"model":"jev-latest","state":state,"questions":questions})
	);
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
async fn system_one_http_contract_and_failure_redaction() {
	use axum::{
		Json, Router,
		http::{HeaderMap, StatusCode},
		response::IntoResponse,
		routing::post,
	};
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/systemone", listener.local_addr().unwrap());
	let app = Router::new().route(
		"/systemone",
		post(|headers: HeaderMap, Json(body): Json<Value>| async move {
			assert_eq!(headers["authorization"], "Bearer fixture-key");
			assert_eq!(body["model"], "jev-test");
			assert_eq!(body["questions"]["q"]["type"], "noul");
			match body["state"].as_str().unwrap() {
				"http-error" => {
					(StatusCode::TOO_MANY_REQUESTS, "private provider detail").into_response()
				}
				"malformed" => (StatusCode::OK, "not json").into_response(),
				"missing" => Json(json!({"other":{}})).into_response(),
				_ => Json(json!({"answers":{"q":{"noul":0.75}}})).into_response(),
			}
		}),
	);
	let _server = TestServer(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap()
	}));
	let client = JevClient::new(
		reqwest::Client::builder()
			.timeout(std::time::Duration::from_secs(5))
			.build()
			.unwrap(),
		endpoint,
		"jev-test".into(),
		"AIDASH_SECRET_JEV".into(),
		Arc::new(FixtureCredentials),
	)
	.unwrap();
	let questions = json!({"q":{"type":"noul","instructions":"keep?"}})
		.as_object()
		.unwrap()
		.clone();
	let response = client
		.send(&json!("ok"), &questions, "fixture-key")
		.await
		.unwrap();
	assert_eq!(probability(&response, "q").unwrap(), 0.75);
	for state in ["http-error", "malformed", "missing"] {
		let error = client
			.send(&json!(state), &questions, "fixture-key")
			.await
			.unwrap_err();
		assert!(!error.to_string().contains("private provider detail"));
		assert!(!error.to_string().contains("fixture-key"));
	}
}
#[rstest::rstest]
#[tokio::test]
async fn bounded_transport_rejects_oversize_requests_responses_and_redirects() {
	use axum::{Router, response::IntoResponse, routing::post};
	use std::sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	};
	let requests = Arc::new(AtomicUsize::new(0));
	let seen = requests.clone();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let app = Router::new()
		.route(
			"/oversize",
			post(move || {
				let seen = seen.clone();
				async move {
					seen.fetch_add(1, Ordering::SeqCst);
					"x".repeat(129)
				}
			}),
		)
		.route(
			"/redirect",
			post(|| async {
				(
					axum::http::StatusCode::TEMPORARY_REDIRECT,
					[("location", "/oversize")],
				)
					.into_response()
			}),
		);
	let _server = TestServer(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap()
	}));
	let client = reqwest::Client::builder()
		.redirect(reqwest::redirect::Policy::none())
		.build()
		.unwrap();
	let mut transport = JevClient::new(
		client,
		format!("{endpoint}/oversize"),
		"fixture".into(),
		"AIDASH_SECRET_TEST".into(),
		Arc::new(FixtureCredentials),
	)
	.unwrap();
	transport.max_request_bytes = 1024;
	transport.max_response_bytes = 128;
	transport.max_questions = 1;
	let questions = json!({"q":{"type":"noul"}}).as_object().unwrap().clone();
	assert!(
		transport
			.send(&json!("x".repeat(1024)), &questions, "fixture")
			.await
			.unwrap_err()
			.to_string()
			.contains("request exceeds")
	);
	let mut excess = questions.clone();
	excess.insert("another".into(), json!({"type":"noul"}));
	assert!(
		transport
			.send(&json!({}), &excess, "fixture")
			.await
			.is_err()
	);
	assert_eq!(requests.load(Ordering::SeqCst), 0);
	assert!(
		transport
			.send(&json!({}), &questions, "fixture")
			.await
			.unwrap_err()
			.to_string()
			.contains("response exceeds")
	);
	assert_eq!(requests.load(Ordering::SeqCst), 1);
	transport.endpoint = format!("{endpoint}/redirect");
	assert!(
		transport
			.send(&json!({}), &questions, "fixture")
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
