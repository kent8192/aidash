use super::*;
use crate::federation::tests::{Server, TestKey, route, serve, transport, verified};
use aidash_application::ports::federation::dependencies::Check;
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
use std::sync::{Arc, Mutex};

type Calls = Arc<Mutex<Vec<(HeaderMap, Value)>>>;
async fn receive(
	Path(mode): Path<String>,
	State(calls): State<Calls>,
	headers: HeaderMap,
	Json(input): Json<Value>,
) -> Response {
	calls.lock().unwrap().push((headers, input.clone()));
	match mode.as_str() {
		"reject" => StatusCode::FORBIDDEN.into_response(),
		"unauthorized" => StatusCode::UNAUTHORIZED.into_response(),
		"missing" => StatusCode::NOT_FOUND.into_response(),
		"server-error" => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
		"unavailable" => StatusCode::SERVICE_UNAVAILABLE.into_response(),
		"not-visible" => Json(json!({"visible":false,"pending":[]})).into_response(),
		"malformed" => "not JSON".into_response(),
		"oversize" => Response::new(Body::from(vec![b' '; 131073])),
		"late-body" => Response::new(Body::from_stream(futures_util::stream::once(async {
			tokio::time::sleep(Duration::from_millis(200)).await;
			Ok::<_, std::io::Error>(r#"{"visible":true,"pending":[]}"#)
		}))),
		_ => Json(json!({"visible":true,"pending":[input["reference"]]})).into_response(),
	}
}
async fn fixture(mode: &str) -> (Server, HttpTransport, Route, Input, Arc<TestKey>, Calls) {
	let calls = Arc::new(Mutex::new(vec![]));
	let app = Router::new()
		.route(
			"/scenario/{mode}/federation/v0.1/scoped/dependencies/verify",
			post(receive),
		)
		.with_state(calls.clone());
	let (server, origin) = serve(app).await;
	let key = TestKey::new();
	let client = transport(&key, &origin);
	let route = route(&format!("{origin}/scenario/{mode}/"), Some("mapped-reader"));
	let input = Input {
		reference: Reference::Registry {
			node_id: route.target.to_string(),
			id: "agent".into(),
			version: "1.0.0".into(),
			digest: "approved".into(),
		},
	};
	(server, client, route, input, key, calls)
}

#[rstest]
#[tokio::test]
async fn dependency_rpc_signs_each_call_as_the_mapped_reader() {
	let (_server, client, route, input, key, calls) = fixture("ok").await;
	let first = client.check(&route, &input, Duration::from_secs(20)).await;
	let second = client.check(&route, &input, Duration::from_secs(20)).await;
	let (Check::Confirmed(first), Check::Confirmed(second)) = (first, second) else {
		panic!("both answers confirm")
	};
	assert!(first.visible && second.visible);
	assert_eq!(first.pending, vec![input.reference.clone()]);
	assert_eq!(second.pending, first.pending);
	let calls = calls.lock().unwrap();
	assert_eq!(calls.len(), 2);
	for (headers, body) in calls.iter() {
		assert_eq!(headers["x-aidash-protocol"], "0.5");
		let claims = verified(
			&key,
			headers,
			"POST",
			"/scoped/dependencies/verify",
			&serde_json::to_vec(body).unwrap(),
		);
		assert_eq!(claims.subject, "mapped-reader");
		assert_eq!(body, &json!({"reference":input.reference}));
	}
	assert_ne!(calls[0].0["authorization"], calls[1].0["authorization"]);
}

#[rstest]
#[case::forbidden("reject", false)]
#[case::unauthorized("unauthorized", false)]
#[case::missing("missing", false)]
#[case::server_error("server-error", true)]
#[case::unavailable("unavailable", true)]
#[case::malformed("malformed", true)]
#[case::oversize("oversize", true)]
#[tokio::test]
async fn a_refusal_is_not_a_failure_to_answer(#[case] mode: &str, #[case] unanswered: bool) {
	let (_server, client, route, input, _keys, calls) = fixture(mode).await;
	let check = client.check(&route, &input, Duration::from_secs(20)).await;
	if unanswered {
		assert!(matches!(check, Check::Unavailable), "{check:?}");
	} else {
		assert!(matches!(check, Check::Refused), "{check:?}");
	}
	assert_eq!(calls.lock().unwrap().len(), 1);
}

#[rstest]
#[tokio::test]
async fn an_answer_of_not_visible_is_confirmed_as_such() {
	let (_server, client, route, input, _keys, _calls) = fixture("not-visible").await;
	let Check::Confirmed(checked) = client.check(&route, &input, Duration::from_secs(20)).await
	else {
		panic!("a 200 answers")
	};
	assert!(!checked.visible);
}

#[rstest]
#[tokio::test]
async fn a_disabled_deployment_sends_nothing_and_refuses() {
	let (_server, mut client, route, input, _key, calls) = fixture("ok").await;
	client.egress.enabled = false;
	assert!(matches!(
		client.check(&route, &input, Duration::from_secs(20)).await,
		Check::Refused
	));
	assert!(calls.lock().unwrap().is_empty());
}

#[rstest]
#[tokio::test]
async fn remaining_graph_deadline_bounds_the_entire_response_body() {
	let (_server, client, route, input, _keys, calls) = fixture("late-body").await;
	let result = tokio::time::timeout(
		Duration::from_secs(1),
		client.check(&route, &input, Duration::from_millis(50)),
	)
	.await
	.expect("graph deadline must stop a stalled body");
	assert!(matches!(result, Check::Unavailable));
	assert_eq!(calls.lock().unwrap().len(), 1);
}
