use super::*;
use aidash_application::{Error, Result, ports::Credentials};
use aidash_domain::federation::dependencies::Reference;
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
		assert_eq!(reference, "FIXTURE_KEY");
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
		input.clone(),
	));
	match mode.as_str() {
		"reject" => StatusCode::FORBIDDEN.into_response(),
		"malformed" => "not JSON".into_response(),
		"oversize" => Response::new(Body::from(vec![b' '; 131073])),
		"late-body" => Response::new(Body::from_stream(futures_util::stream::once(async {
			tokio::time::sleep(Duration::from_millis(200)).await;
			Ok::<_, std::io::Error>(r#"{"visible":true,"pending":[]}"#)
		}))),
		_ => Json(json!({"visible":true,"pending":[input["reference"]]})).into_response(),
	}
}
async fn fixture(mode: &str) -> (Server, PeerHttp, Peer, Input, Arc<Keys>, Calls) {
	let calls = Arc::new(Mutex::new(vec![]));
	let app = Router::new()
		.route(
			"/scenario/{mode}/federation/v0.1/scoped/dependencies/verify",
			post(receive),
		)
		.with_state(calls.clone());
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/scenario/{mode}/", listener.local_addr().unwrap());
	let server = Server(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap()
	}));
	let keys = Arc::new(Keys(RwLock::new(Some("before-rotation".into()))));
	let client = PeerHttp {
		client: reqwest::Client::builder()
			.redirect(reqwest::redirect::Policy::none())
			.build()
			.unwrap(),
		node_id: "aidash://home".into(),
		protocol_version: "0.1".into(),
		credentials: keys.clone(),
	};
	let peer = Peer {
		node_id: "aidash://leaf".into(),
		endpoint,
		credential_env: "FIXTURE_KEY".into(),
		protocol_version: "0.1".into(),
		enabled: true,
	};
	let input = Input {
		tenant: "mapped-tenant".into(),
		subject: "mapped-reader".into(),
		reference: Reference::Registry {
			node_id: peer.node_id.clone(),
			id: "agent".into(),
			version: "1.0.0".into(),
			digest: "approved".into(),
		},
	};
	(server, client, peer, input, keys, calls)
}

#[rstest]
#[tokio::test]
async fn dependency_rpc_preserves_mapped_identity_headers_and_key_rotation() {
	let (_server, client, peer, input, keys, calls) = fixture("ok").await;
	let first = client
		.check(&peer, &input, Duration::from_secs(20))
		.await
		.unwrap();
	*keys.0.write().unwrap() = Some("after-rotation".into());
	let second = client
		.check(&peer, &input, Duration::from_secs(20))
		.await
		.unwrap();
	assert!(first.visible && second.visible);
	assert_eq!(first.pending, vec![input.reference.clone()]);
	assert_eq!(second.pending, first.pending);
	let calls = calls.lock().unwrap();
	assert_eq!(calls.len(), 2);
	assert_eq!(
		calls[0],
		(
			"Bearer before-rotation".into(),
			"aidash://home".into(),
			"0.1".into(),
			serde_json::to_value(&input).unwrap()
		)
	);
	assert_eq!(calls[1].0, "Bearer after-rotation");
}

#[rstest]
#[case("reject")]
#[case("malformed")]
#[case("oversize")]
#[tokio::test]
async fn invalid_remote_proof_denies_visibility(#[case] mode: &str) {
	let (_server, client, peer, input, _keys, calls) = fixture(mode).await;
	assert!(
		client
			.check(&peer, &input, Duration::from_secs(20))
			.await
			.is_none()
	);
	assert_eq!(calls.lock().unwrap().len(), 1);
}

#[rstest]
#[tokio::test]
async fn unavailable_credentials_prevent_any_peer_request() {
	let (_server, client, peer, input, keys, calls) = fixture("ok").await;
	*keys.0.write().unwrap() = None;
	assert!(
		client
			.check(&peer, &input, Duration::from_secs(20))
			.await
			.is_none()
	);
	assert!(calls.lock().unwrap().is_empty());
}

#[rstest]
#[tokio::test]
async fn remaining_graph_deadline_bounds_the_entire_response_body() {
	let (_server, client, peer, input, _keys, calls) = fixture("late-body").await;
	let result = tokio::time::timeout(
		Duration::from_secs(1),
		client.check(&peer, &input, Duration::from_millis(50)),
	)
	.await
	.expect("graph deadline must stop a stalled body");
	assert!(result.is_none());
	assert_eq!(calls.lock().unwrap().len(), 1);
}
