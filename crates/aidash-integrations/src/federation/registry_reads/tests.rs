use super::*;
use crate::federation::tests::{Server, TestKey, route, serve, transport, verified};
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
use std::sync::{Arc, Mutex};
type Calls = Arc<Mutex<Vec<(HeaderMap, Value)>>>;
async fn receive(
	Path(mode): Path<String>,
	State(calls): State<Calls>,
	headers: HeaderMap,
	Json(input): Json<Value>,
) -> Response {
	calls.lock().unwrap().push((headers, input));
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
async fn fixture(
	mode: &str,
) -> (
	Server,
	HttpTransport,
	Route,
	Vec<Reference>,
	Arc<TestKey>,
	Calls,
) {
	let calls = Arc::new(Mutex::new(vec![]));
	let app = Router::new()
		.route(
			"/scenario/{mode}/federation/v0.1/scoped/registry/verify",
			post(receive),
		)
		.with_state(calls.clone());
	let (server, origin) = serve(app).await;
	let key = TestKey::new();
	let transport = transport(&key, &origin);
	let peer = route(&format!("{origin}/scenario/{mode}/"), Some("mapped-reader"));
	let references = vec![Reference {
		entry: EntityRef {
			id: "agent".into(),
			version: "1.0.0".into(),
		},
		digest: format!("sha256:{}", "a".repeat(64)),
	}];
	(server, transport, peer, references, key, calls)
}
#[rstest]
#[tokio::test]
async fn registry_rpc_signs_each_call_as_the_mapped_reader() {
	let (_server, transport, peer, references, key, calls) = fixture("ok").await;
	for _ in 0..2 {
		assert_eq!(
			transport.verify(&peer, &references).await,
			Verification::Verified
		);
	}
	let calls = calls.lock().unwrap();
	assert_eq!(calls.len(), 2);
	for (headers, body) in calls.iter() {
		assert_eq!(headers["x-aidash-protocol"], "0.4");
		let claims = verified(
			&key,
			headers,
			"POST",
			"/scoped/registry/verify",
			&serde_json::to_vec(body).unwrap(),
		);
		assert_eq!(claims.subject, "mapped-reader");
		assert_eq!(body, &json!({"references":references}));
	}
	assert_ne!(calls[0].0["authorization"], calls[1].0["authorization"]);
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
	assert_eq!(transport.verify(&peer, &references).await, expected);
	assert_eq!(calls.lock().unwrap().len(), 1);
}
#[rstest]
#[tokio::test]
async fn a_disabled_deployment_sends_nothing() {
	let (_server, mut transport, peer, references, _key, calls) = fixture("ok").await;
	transport.egress.enabled = false;
	assert_eq!(
		transport.verify(&peer, &references).await,
		Verification::Unavailable
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
	assert_eq!(transport.verify(&peer, &references).await, expected);
	assert!(started.elapsed() >= Duration::from_secs(9));
	assert!(started.elapsed() < Duration::from_secs(11));
	assert_eq!(calls.lock().unwrap().len(), 1);
}
