use super::*;
use aidash_application::ports::memory::objects::RecoveryObjects;
use axum::{
	Router,
	body::{Body, Bytes},
	extract::{Query, State},
	http::{HeaderMap, StatusCode, Uri},
	response::{IntoResponse, Response},
	routing::any,
};
use serde_json::json;
use std::{
	collections::BTreeMap,
	sync::{Arc, Mutex},
};

#[derive(Default)]
struct Fixture {
	objects: BTreeMap<String, (String, Vec<u8>)>,
	next: u64,
}
struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
	fn drop(&mut self) {
		self.0.abort();
	}
}

async fn serve(
	State(state): State<Arc<Mutex<Fixture>>>,
	method: axum::http::Method,
	uri: Uri,
	Query(query): Query<BTreeMap<String, String>>,
	headers: HeaderMap,
	bytes: Bytes,
) -> Response {
	if uri.path() == "/token" {
		if headers
			.get("metadata-flavor")
			.and_then(|value| value.to_str().ok())
			!= Some("Google")
		{
			return StatusCode::FORBIDDEN.into_response();
		}
		return axum::Json(json!({"access_token":"recovery-canary-token","expires_in":3600}))
			.into_response();
	}
	if headers
		.get("authorization")
		.and_then(|value| value.to_str().ok())
		!= Some("Bearer recovery-canary-token")
	{
		return StatusCode::UNAUTHORIZED.into_response();
	}
	let mut state = state.lock().unwrap();
	let base = "/storage/v1/b/fixture-bucket/o";
	if method == "POST" && uri.path() == "/upload/storage/v1/b/fixture-bucket/o" {
		if query.get("ifGenerationMatch").map(String::as_str) != Some("0")
			|| query.get("uploadType").map(String::as_str) != Some("media")
		{
			return StatusCode::BAD_REQUEST.into_response();
		}
		let name = query.get("name").unwrap().clone();
		if name == "denied" {
			return (StatusCode::FORBIDDEN, "recovery-canary-token").into_response();
		}
		if state.objects.contains_key(&name) {
			return StatusCode::PRECONDITION_FAILED.into_response();
		}
		state.next += 1;
		let generation = state.next.to_string();
		state
			.objects
			.insert(name, (generation.clone(), bytes.to_vec()));
		return axum::Json(json!({"generation":generation})).into_response();
	}
	if method == "GET" && uri.path() == base {
		let prefix = query.get("prefix").map(String::as_str).unwrap_or("");
		let offset = query
			.get("pageToken")
			.and_then(|value| value.parse::<usize>().ok())
			.unwrap_or(0);
		let matching = state
			.objects
			.iter()
			.filter(|(name, _)| name.starts_with(prefix))
			.collect::<Vec<_>>();
		let items = matching
			.iter()
			.skip(offset)
			.take(1)
			.map(|(name, (generation, _))| json!({"name":name,"generation":generation}))
			.collect::<Vec<_>>();
		return axum::Json(json!({"items":items,"nextPageToken":if offset+1<matching.len() { Some((offset+1).to_string()) } else { None }})).into_response();
	}
	let Some(encoded) = uri.path().strip_prefix(&format!("{base}/")) else {
		return StatusCode::NOT_FOUND.into_response();
	};
	let name = percent_encoding::percent_decode_str(encoded)
		.decode_utf8()
		.unwrap()
		.into_owned();
	let Some((generation, data)) = state.objects.get(&name).cloned() else {
		return StatusCode::NOT_FOUND.into_response();
	};
	if method == "DELETE" {
		if query.get("ifGenerationMatch") != Some(&generation) {
			return StatusCode::PRECONDITION_FAILED.into_response();
		}
		state.objects.remove(&name);
		return StatusCode::NO_CONTENT.into_response();
	}
	if query.get("alt").map(String::as_str) == Some("media") {
		if query.get("generation") != Some(&generation) {
			return StatusCode::NOT_FOUND.into_response();
		}
		return Response::new(Body::from(data));
	}
	axum::Json(json!({"generation":generation})).into_response()
}

async fn fixture() -> (Server, GcsObjects) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let base = format!("http://{}", listener.local_addr().unwrap());
	let router = Router::new()
		.route("/{*path}", any(serve))
		.with_state(Arc::new(Mutex::new(Fixture::default())));
	let server = Server(tokio::spawn(async move {
		axum::serve(listener, router).await.unwrap();
	}));
	let objects = GcsObjects::at(
		"fixture-bucket".into(),
		base.clone(),
		format!("{base}/token"),
	)
	.unwrap();
	(server, objects)
}

#[tokio::test]
async fn immutable_object_replay_verifies_bytes_and_generation_aware_delete() {
	let (_server, objects) = fixture().await;
	let generation = objects.create("home/one", b"floor-one").await.unwrap();
	assert_eq!(
		objects.create("home/one", b"floor-one").await.unwrap(),
		generation
	);
	assert!(matches!(
		objects.create("home/one", b"different").await,
		Err(crate::Error::Conflict(_))
	));
	let saved = objects
		.get("home/one", Some(&generation))
		.await
		.unwrap()
		.unwrap();
	assert_eq!(saved.bytes, b"floor-one");
	assert!(objects.delete("home/one", "999999").await.is_err());
	assert!(objects.get("home/one", None).await.unwrap().is_some());
	objects.delete("home/one", &generation).await.unwrap();
	assert!(objects.get("home/one", None).await.unwrap().is_none());
}

#[tokio::test]
async fn recovery_listing_keeps_opaque_pages_and_errors_never_reflect_credentials() {
	let (_server, objects) = fixture().await;
	objects.create("home/one", b"one").await.unwrap();
	objects.create("home/two", b"two").await.unwrap();
	objects.create("other/three", b"three").await.unwrap();
	let first = objects.list("home/", None).await.unwrap();
	assert_eq!(
		first
			.objects
			.iter()
			.map(|value| value.name.as_str())
			.collect::<Vec<_>>(),
		["home/one"]
	);
	let second = objects.list("home/", first.next.as_deref()).await.unwrap();
	assert_eq!(
		second
			.objects
			.iter()
			.map(|value| value.name.as_str())
			.collect::<Vec<_>>(),
		["home/two"]
	);
	assert!(second.next.is_none());
	let error = objects
		.create("denied", b"untrusted")
		.await
		.unwrap_err()
		.to_string();
	assert!(!error.contains("recovery-canary-token"));
	assert!(!error.contains("127.0.0.1"));
}
