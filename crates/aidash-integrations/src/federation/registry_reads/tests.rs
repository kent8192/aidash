use super::*;
use aidash_application::{Error, Result, ports::Credentials};
use aidash_domain::registry::EntityRef;
use axum::{
	Json, Router,
	body::Body,
	extract::{Path, State},
	http::{HeaderMap, StatusCode},
	response::{IntoResponse, Response},
	routing::post,
};
use rstest::rstest;
use serde_json::Value;
use std::sync::{Arc, Mutex, RwLock};
struct Keys(RwLock<Option<String>>);
impl Credentials for Keys {
	fn resolve(&self, reference: &str) -> Result<String> {
		assert_eq!(reference, "REGISTRY_FIXTURE_KEY");
		self.0
			.read()
			.unwrap()
			.clone()
			.ok_or_else(|| Error::Invalid("fixture key unavailable".into()))
	}
}
struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
	fn drop(&mut self) {
		self.0.abort();
	}
}
type Calls = Arc<Mutex<Vec<(String, String, String, Value)>>>;
async fn receive(
	Path(mode): Path<String>,
	State(calls): State<Calls>,
	headers: HeaderMap,
	Json(input): Json<Value>,
) -> Response {
	calls.lock().unwrap().push((
		headers["authorization"].to_str().unwrap().into(),
		headers["x-aidash-node"].to_str().unwrap().into(),
		headers["x-aidash-protocol"].to_str().unwrap().into(),
		input,
	));
	match mode.as_str() {
		"negative" => "false".into_response(),
		"status" => StatusCode::FORBIDDEN.into_response(),
		"malformed" => "invalid JSON".into_response(),
		"wrong-type" => Json(json!({"visible":true})).into_response(),
		"exact" => format!("true{}", " ".repeat(1020)).into_response(),
		"oversize" => format!("true{}", " ".repeat(1021)).into_response(),
		"chunks" => Response::new(Body::from_stream(futures_util::stream::iter([
			Ok::<_, std::io::Error>(vec![b' '; 600]),
			Ok(vec![b' '; 600]),
		]))),
		"late-headers" => {
			tokio::time::sleep(Duration::from_secs(11)).await;
			"true".into_response()
		}
		"late-body" => Response::new(Body::from_stream(futures_util::stream::once(async {
			tokio::time::sleep(Duration::from_secs(11)).await;
			Ok::<_, std::io::Error>("true")
		}))),
		_ => "true".into_response(),
	}
}
async fn fixture(mode: &str) -> (Server, PeerHttp, Peer, Vec<Reference>, Arc<Keys>, Calls) {
	let calls = Arc::new(Mutex::new(vec![]));
	let app = Router::new()
		.route(
			"/scenario/{mode}/federation/v0.1/scoped/registry/verify",
			post(receive),
		)
		.with_state(calls.clone());
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/scenario/{mode}/", listener.local_addr().unwrap());
	let server = Server(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap();
	}));
	let keys = Arc::new(Keys(RwLock::new(Some("before-rotation".into()))));
	let transport = PeerHttp {
		client: reqwest::Client::new(),
		node_id: "aidash://local".into(),
		protocol_version: "0.1".into(),
		credentials: keys.clone(),
	};
	let peer = Peer {
		node_id: "aidash://remote".into(),
		endpoint,
		credential_env: "REGISTRY_FIXTURE_KEY".into(),
		protocol_version: "0.1".into(),
		enabled: true,
	};
	let references = vec![Reference {
		entry: EntityRef {
			id: "agent".into(),
			version: "1.0.0".into(),
		},
		digest: format!("sha256:{}", "a".repeat(64)),
	}];
	(server, transport, peer, references, keys, calls)
}
#[rstest]
#[tokio::test]
async fn registry_rpc_preserves_headers_mapped_identity_references_and_rotating_keys() {
	let (_server, transport, peer, references, keys, calls) = fixture("ok").await;
	assert_eq!(
		transport
			.verify(&peer, "mapped-tenant", "mapped-reader", &references)
			.await,
		Verification::Verified
	);
	*keys.0.write().unwrap() = Some("after-rotation".into());
	assert_eq!(
		transport
			.verify(&peer, "mapped-tenant", "mapped-reader", &references)
			.await,
		Verification::Verified
	);
	let calls = calls.lock().unwrap();
	assert_eq!(calls.len(), 2);
	assert_eq!(
		calls[0],
		(
			"Bearer before-rotation".into(),
			"aidash://local".into(),
			"0.1".into(),
			json!({"tenant":"mapped-tenant","subject":"mapped-reader","references":references})
		)
	);
	assert_eq!(calls[1].0, "Bearer after-rotation");
}
#[rstest]
#[case::status("status", Verification::Unavailable)]
#[case::negative("negative", Verification::Rejected)]
#[case::malformed("malformed", Verification::Rejected)]
#[case::wrong_type("wrong-type", Verification::Rejected)]
#[case::limit("exact", Verification::Verified)]
#[case::overflow("oversize", Verification::Rejected)]
#[case::chunked_overflow("chunks", Verification::Rejected)]
#[tokio::test]
async fn response_contract_and_body_limit_retain_the_existing_denial_category(
	#[case] mode: &str,
	#[case] expected: Verification,
) {
	let (_server, transport, peer, references, _keys, calls) = fixture(mode).await;
	assert_eq!(
		transport
			.verify(&peer, "tenant", "reader", &references)
			.await,
		expected
	);
	assert_eq!(calls.lock().unwrap().len(), 1);
}
#[rstest]
#[tokio::test]
async fn absent_credentials_deny_before_any_request() {
	let (_server, transport, peer, references, keys, calls) = fixture("ok").await;
	*keys.0.write().unwrap() = None;
	assert_eq!(
		transport
			.verify(&peer, "tenant", "reader", &references)
			.await,
		Verification::Rejected
	);
	assert!(calls.lock().unwrap().is_empty());
}
#[rstest]
#[case::headers("late-headers", Verification::Unavailable)]
#[case::body("late-body", Verification::Rejected)]
#[tokio::test]
async fn ten_second_deadline_covers_headers_and_body_without_changing_denial_semantics(
	#[case] mode: &str,
	#[case] expected: Verification,
) {
	let (_server, transport, peer, references, _keys, calls) = fixture(mode).await;
	let started = std::time::Instant::now();
	assert_eq!(
		transport
			.verify(&peer, "tenant", "reader", &references)
			.await,
		expected
	);
	assert!(started.elapsed() >= Duration::from_secs(9));
	assert!(started.elapsed() < Duration::from_secs(11));
	assert_eq!(calls.lock().unwrap().len(), 1);
}
