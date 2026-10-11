use super::*;
use axum::{Json, Router, http::StatusCode, routing::post};
use serde_json::Value;
use std::sync::{Arc, Mutex};

struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
	fn drop(&mut self) {
		self.0.abort();
	}
}
async fn serve(app: Router) -> (String, Server) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let url = format!("http://{}", listener.local_addr().unwrap());
	(
		url,
		Server(tokio::spawn(async move {
			axum::serve(listener, app).await.unwrap();
		})),
	)
}

#[tokio::test]
async fn exchange_posts_the_code_and_verifier_and_returns_the_issued_key() {
	let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
	let history = seen.clone();
	let app = Router::new().route(
		"/api/v1/auth/keys",
		post(move |Json(body): Json<Value>| {
			let history = history.clone();
			async move {
				history.lock().unwrap().push(body);
				Json(serde_json::json!({"key":"sk-or-v1-issued","user_id":"user"}))
			}
		}),
	);
	let (url, _server) = serve(app).await;
	let exchange = OpenRouterAuthorization::at(reqwest::Client::new(), format!("{url}/api/v1"));
	let key = exchange
		.exchange(Provider::Openrouter, "code-1", &"verifier-1".into())
		.await
		.unwrap();
	assert_eq!(key.expose_secret(), "sk-or-v1-issued");
	assert_eq!(
		seen.lock().unwrap().as_slice(),
		[
			serde_json::json!({"code":"code-1","code_verifier":"verifier-1","code_challenge_method":"S256"})
		]
	);
}

#[tokio::test]
async fn rejected_codes_fail_without_echoing_the_provider_body() {
	for (status, rejected) in [
		(StatusCode::FORBIDDEN, true),
		(StatusCode::BAD_REQUEST, true),
		(StatusCode::INTERNAL_SERVER_ERROR, false),
		(StatusCode::TOO_MANY_REQUESTS, false),
	] {
		let app = Router::new().route(
			"/auth/keys",
			post(move || async move { (status, "error-body-canary sk-or-v1-leak") }),
		);
		let (url, _server) = serve(app).await;
		let error = OpenRouterAuthorization::at(reqwest::Client::new(), url)
			.exchange(Provider::Openrouter, "code", &"verifier".into())
			.await
			.unwrap_err();
		let text = format!("{error:?} {error}");
		assert!(
			!text.contains("canary") && !text.contains("sk-or"),
			"{text}"
		);
		assert_eq!(matches!(error, Error::Invalid(_)), rejected, "{status}");
	}
}

#[tokio::test]
async fn empty_or_oversized_codes_are_refused_locally() {
	let exchange = OpenRouterAuthorization::at(reqwest::Client::new(), "http://127.0.0.1:9".into());
	for code in [String::new(), "c".repeat(1025)] {
		assert!(matches!(
			exchange
				.exchange(Provider::Openrouter, &code, &"verifier".into())
				.await,
			Err(Error::Invalid(_))
		));
	}
}
