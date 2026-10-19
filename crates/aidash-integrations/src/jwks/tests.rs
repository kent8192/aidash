use super::*;
use serde_json::{Value, json};
use std::sync::Arc;

struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
	fn drop(&mut self) {
		self.0.abort();
	}
}

#[tokio::test]
async fn jwks_cache_respects_expiry_rotation_and_unknown_key_throttling() {
	use axum::{Json, Router, routing::get};
	use std::sync::atomic::{AtomicUsize, Ordering};
	let requests = Arc::new(AtomicUsize::new(0));
	let count = requests.clone();
	let mut jwks: Value =
		serde_json::from_str(include_str!("signing-test-only.jwks.json")).unwrap();
	jwks["keys"][0]["kid"] = json!("fixture");
	let body = Arc::new(std::sync::Mutex::new(jwks));
	let response = body.clone();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let url = format!("http://{}/keys", listener.local_addr().unwrap());
	let router = Router::new().route(
		"/keys",
		get(move || {
			let value = response.lock().unwrap().clone();
			count.fetch_add(1, Ordering::SeqCst);
			async { ([("cache-control", "max-age=60")], Json(value)) }
		}),
	);
	let _server = Server(tokio::spawn(async move {
		axum::serve(listener, router).await.unwrap()
	}));
	let keys = HttpSigningKeys::new(reqwest::Client::new(), url);
	keys.key("fixture").await.unwrap();
	keys.key("fixture").await.unwrap();
	assert!(keys.key("unknown").await.is_err());
	assert_eq!(requests.load(Ordering::SeqCst), 1);
	keys.cache.lock().await.as_mut().unwrap().fetched = Instant::now() - Duration::from_secs(61);
	keys.key("fixture").await.unwrap();
	assert_eq!(requests.load(Ordering::SeqCst), 2);
	body.lock().unwrap()["keys"][0]["kid"] = json!("rotated");
	keys.cache.lock().await.as_mut().unwrap().fetched = Instant::now() - Duration::from_secs(31);
	keys.key("rotated").await.unwrap();
	assert_eq!(requests.load(Ordering::SeqCst), 3);
}
