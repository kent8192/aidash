use super::*;
use aidash_domain::keyless::PermittedOrigins;
use axum::{Json, Router, response::Redirect, routing::get};
use serde_json::json;

async fn serve(app: Router) -> (String, tokio::task::JoinHandle<()>) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	(
		endpoint,
		tokio::spawn(async move { axum::serve(listener, app).await.unwrap() }),
	)
}

fn identity() -> Router {
	Router::new().route(
		"/.well-known/aidash",
		get(|| async { Json(json!({"id":"aidash://remote","protocol_version":"0.4"})) }),
	)
}

fn directory(permitted: &[&str]) -> HttpDirectory {
	HttpDirectory::new(KeylessAccess::permitting(
		PermittedOrigins::parse(permitted).unwrap(),
	))
	.unwrap()
}

fn unprocessable(result: Result<Value>) -> String {
	match result {
		Err(Error::Unprocessable(reason)) => reason,
		other => panic!("expected the generic refusal, got {other:?}"),
	}
}

#[tokio::test]
async fn a_loopback_endpoint_is_refused_unless_its_origin_is_permitted() {
	let (endpoint, _server) = serve(identity()).await;
	assert_eq!(
		unprocessable(directory(&[]).identity(&endpoint).await),
		UNREACHABLE
	);
}

#[tokio::test]
async fn a_permitted_private_origin_is_fetched() {
	let (endpoint, _server) = serve(identity()).await;
	let document = directory(&[&endpoint]).identity(&endpoint).await.unwrap();
	assert_eq!(document["id"], "aidash://remote");
}

#[tokio::test]
async fn a_redirect_is_not_followed_even_from_a_permitted_origin() {
	let target = Router::new().route(
		"/.well-known/aidash",
		get(|| async { Redirect::temporary("/elsewhere") }),
	);
	let (endpoint, _server) = serve(target).await;
	assert_eq!(
		unprocessable(directory(&[&endpoint]).identity(&endpoint).await),
		UNREACHABLE
	);
}

#[tokio::test]
async fn a_public_endpoint_must_use_https() {
	assert_eq!(
		unprocessable(directory(&[]).identity("http://93.184.216.34").await),
		UNREACHABLE
	);
}

#[tokio::test]
async fn a_closed_port_a_server_error_and_malformed_json_answer_the_same() {
	let closed = {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		format!("http://{}", listener.local_addr().unwrap())
	};
	let failing = Router::new().route(
		"/.well-known/aidash",
		get(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }),
	);
	let (failing, _a) = serve(failing).await;
	let garbage = Router::new().route("/.well-known/aidash", get(|| async { "not json" }));
	let (garbage, _b) = serve(garbage).await;
	let directory = directory(&[&closed, &failing, &garbage]);
	let answers = [
		unprocessable(directory.identity(&closed).await),
		unprocessable(directory.identity(&failing).await),
		unprocessable(directory.identity(&garbage).await),
		unprocessable(directory.identity("not a url").await),
	];
	assert!(
		answers.iter().all(|answer| answer == UNREACHABLE),
		"{answers:?}"
	);
}
