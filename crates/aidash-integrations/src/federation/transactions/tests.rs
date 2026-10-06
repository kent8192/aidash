use super::*;
use aidash_application::ports::Credentials;
use axum::{
	Router,
	body::{Body, Bytes},
	extract::{Path, State},
	http::{HeaderMap, Method, StatusCode},
	response::{IntoResponse, Response},
	routing::any,
};
use rstest::rstest;
use serde_json::{Value, json};
use std::sync::{
	Arc, Mutex, RwLock,
	atomic::{AtomicBool, Ordering},
};

struct Keys(RwLock<Option<String>>);
impl Credentials for Keys {
	fn resolve(&self, reference: &str) -> Result<String> {
		assert_eq!(reference, "TRANSACTION_FIXTURE_KEY");
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
type Calls = Arc<Mutex<Vec<(String, String, String, String, Value)>>>;

async fn receive(
	Path(mode): Path<String>,
	State(calls): State<Calls>,
	method: Method,
	headers: HeaderMap,
	body: Bytes,
) -> Response {
	let input = if body.is_empty() {
		Value::Null
	} else {
		serde_json::from_slice(&body).unwrap()
	};
	calls.lock().unwrap().push((
		method.to_string(),
		headers["authorization"].to_str().unwrap().into(),
		headers["x-aidash-node"].to_str().unwrap().into(),
		headers["x-aidash-protocol"].to_str().unwrap().into(),
		input,
	));
	if let Some(status) = mode.strip_prefix("status-") {
		let (status, pending) = status.split_once('_').unwrap();
		// Error bodies are intentionally not protocol JSON and are not decoded.
		let mut response = (
			StatusCode::from_u16(status.parse().unwrap()).unwrap(),
			"untrusted error body",
		)
			.into_response();
		if pending != "none" {
			response
				.headers_mut()
				.insert("x-aidash-transaction-pending", pending.parse().unwrap());
		}
		return response;
	}
	match mode.as_str() {
		"malformed" => "invalid JSON".into_response(),
		"exact" => format!("{{}}{}", " ".repeat(4_194_302)).into_response(),
		"oversize" => format!("{{}}{}", " ".repeat(4_194_303)).into_response(),
		"chunks" => Response::new(Body::from_stream(futures_util::stream::iter([
			Ok::<_, std::io::Error>(vec![b' '; 2_097_152]),
			Ok(vec![b' '; 2_097_153]),
		]))),
		"late-headers" => {
			tokio::time::sleep(Duration::from_secs(11)).await;
			"{}".into_response()
		}
		"late-body" => Response::new(Body::from_stream(futures_util::stream::once(async {
			tokio::time::sleep(Duration::from_secs(11)).await;
			Ok::<_, std::io::Error>("{}")
		}))),
		"late-after-lookup" => {
			Response::new(Body::from_stream(futures_util::stream::once(async {
				tokio::time::sleep(Duration::from_secs(5)).await;
				Ok::<_, std::io::Error>("{}")
			})))
		}
		_ => "{\"revision\":42}".into_response(),
	}
}

async fn fixture(mode: &str) -> (Server, PeerHttp, Peer, Arc<Keys>, Calls) {
	let calls = Arc::new(Mutex::new(vec![]));
	let app = Router::new()
		.route("/scenario/{mode}/federation/v0.1/rpc", any(receive))
		.with_state(calls.clone());
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/scenario/{mode}/", listener.local_addr().unwrap());
	let server = Server(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap();
	}));
	let keys = Arc::new(Keys(RwLock::new(Some("before-rotation".into()))));
	let transport = PeerHttp {
		client: reqwest::Client::builder()
			.redirect(reqwest::redirect::Policy::none())
			.build()
			.unwrap(),
		node_id: "aidash://local".into(),
		protocol_version: "0.1".into(),
		credentials: keys.clone(),
	};
	let peer = Peer {
		node_id: "aidash://remote".into(),
		endpoint,
		credential_env: "TRANSACTION_FIXTURE_KEY".into(),
		protocol_version: "0.1".into(),
		enabled: true,
	};
	(server, transport, peer, keys, calls)
}

#[rstest]
#[tokio::test]
async fn transaction_rpc_preserves_headers_json_and_rotating_credentials() {
	let (_server, transport, peer, keys, calls) = fixture("ok").await;
	let first: Value = transport
		.transaction(
			async { Ok(peer.clone()) },
			"POST",
			"/rpc",
			Some(&json!({"manifest":"first"})),
		)
		.await
		.unwrap();
	*keys.0.write().unwrap() = Some("after-rotation".into());
	let second: Value = transport
		.transaction(async { Ok(peer) }, "GET", "/rpc", None::<&()>)
		.await
		.unwrap();
	assert_eq!(first, json!({"revision":42}));
	assert_eq!(second, first);
	assert_eq!(
		*calls.lock().unwrap(),
		[
			(
				"POST".into(),
				"Bearer before-rotation".into(),
				"aidash://local".into(),
				"0.1".into(),
				json!({"manifest":"first"})
			),
			(
				"GET".into(),
				"Bearer after-rotation".into(),
				"aidash://local".into(),
				"0.1".into(),
				Value::Null
			),
		]
	);
}

#[rstest]
#[case::bad_request(400, "none", "invalid")]
#[case::unprocessable(422, "none", "invalid")]
#[case::not_found(404, "none", "invalid")]
#[case::method(405, "none", "invalid")]
#[case::unauthorized(401, "none", "unauthorized")]
#[case::forbidden(403, "none", "forbidden")]
#[case::conflict(409, "none", "conflict")]
#[case::pending(503, "1", "pending")]
#[case::unavailable(503, "none", "external")]
#[case::wrong_header(503, "2", "external")]
#[case::wrong_status(500, "1", "external")]
#[case::limited(429, "none", "external")]
#[tokio::test]
async fn participant_status_keeps_its_error_identity_without_reading_the_error_body(
	#[case] status: u16,
	#[case] pending: &str,
	#[case] category: &str,
) {
	let (_server, transport, peer, _keys, calls) =
		fixture(&format!("status-{status}_{pending}")).await;
	let error = transport
		.transaction::<Value>(async { Ok(peer) }, "POST", "/rpc", Some(&json!({})))
		.await
		.unwrap_err();
	let status = StatusCode::from_u16(status).unwrap();
	match (category, error) {
		("invalid", Error::Invalid(message)) => assert_eq!(
			message,
			format!("transaction participant rejected request: {status}")
		),
		("unauthorized", Error::Unauthorized)
		| ("forbidden", Error::Forbidden)
		| ("pending", Error::TransactionPending) => {}
		("conflict", Error::Conflict(message)) => assert_eq!(
			message,
			"transaction participant rejected its state precondition"
		),
		("external", Error::External(message)) => assert_eq!(
			message,
			format!("transaction participant returned {status}")
		),
		(_, error) => panic!("wrong transaction error category: {error:?}"),
	}
	assert_eq!(calls.lock().unwrap().len(), 1);
}

#[rstest]
#[case::exact("exact", None)]
#[case::oversize("oversize", Some("response exceeds 4194304 bytes"))]
#[case::chunks("chunks", Some("response exceeds 4194304 bytes"))]
#[case::malformed("malformed", Some("invalid response JSON:"))]
#[tokio::test]
async fn complete_body_enforces_the_existing_four_mib_json_bound(
	#[case] mode: &str,
	#[case] message: Option<&str>,
) {
	let (_server, transport, peer, _keys, calls) = fixture(mode).await;
	let result = transport
		.transaction::<Value>(async { Ok(peer) }, "GET", "/rpc", None::<&()>)
		.await;
	match message {
		None => assert_eq!(result.unwrap(), json!({})),
		Some(message) => {
			assert!(matches!(result, Err(Error::External(error)) if error.starts_with(message)))
		}
	}
	assert_eq!(calls.lock().unwrap().len(), 1);
}

#[rstest]
#[case::headers("late-headers", 0)]
#[case::body("late-body", 0)]
#[case::lookup_then_body("late-after-lookup", 6)]
#[tokio::test]
async fn lookup_headers_and_body_share_one_ten_second_deadline(
	#[case] mode: &str,
	#[case] lookup_seconds: u64,
) {
	let (_server, transport, peer, _keys, calls) = fixture(mode).await;
	let started = std::time::Instant::now();
	let result = transport
		.transaction::<Value>(
			async {
				tokio::time::sleep(Duration::from_secs(lookup_seconds)).await;
				Ok(peer)
			},
			"GET",
			"/rpc",
			None::<&()>,
		)
		.await;
	// The inherited request timer can win the outer deadline for headers or body.
	// Both retained timeout outcomes remain external failures, never definitive rejection.
	assert!(
		matches!(&result, Err(Error::External(message)) if message == "transaction participant response timed out; outcome retained for recovery"
			|| (mode == "late-headers" && message == "error sending request")
			|| (mode == "late-body" && message == "error decoding response body")),
		"unexpected timeout result: {result:?}"
	);
	assert!(started.elapsed() >= Duration::from_secs(9));
	assert!(started.elapsed() < Duration::from_secs(11));
	assert_eq!(calls.lock().unwrap().len(), 1);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn slow_peer_lookup_expires_before_sending() {
	let (_server, transport, peer, _keys, calls) = fixture("ok").await;
	let started = tokio::time::Instant::now();
	let result = transport
		.transaction::<Value>(
			async {
				tokio::time::sleep(Duration::from_secs(11)).await;
				Ok(peer)
			},
			"GET",
			"/rpc",
			None::<&()>,
		)
		.await;
	assert!(
		matches!(result, Err(Error::External(message)) if message == "transaction participant response timed out; outcome retained for recovery")
	);
	assert_eq!(started.elapsed(), Duration::from_secs(10));
	assert!(calls.lock().unwrap().is_empty());
}

#[rstest]
#[tokio::test]
async fn peer_lookup_and_missing_keys_fail_without_contacting_the_peer() {
	let (_server, transport, peer, keys, calls) = fixture("ok").await;
	let result = transport
		.transaction::<Value>(
			async { Err(Error::NotFound("peer disabled".into())) },
			"GET",
			"/rpc",
			None::<&()>,
		)
		.await;
	assert!(matches!(result, Err(Error::NotFound(message)) if message == "peer disabled"));
	*keys.0.write().unwrap() = None;
	let result = transport
		.transaction::<Value>(async { Ok(peer) }, "GET", "/rpc", None::<&()>)
		.await;
	assert!(matches!(result, Err(Error::Invalid(message)) if message == "fixture key unavailable"));
	assert!(calls.lock().unwrap().is_empty());
}

#[rstest]
#[tokio::test]
async fn serialization_still_precedes_lookup_and_the_response_deadline() {
	struct Unserializable;
	impl Serialize for Unserializable {
		fn serialize<S: serde::Serializer>(
			&self,
			_serializer: S,
		) -> std::result::Result<S::Ok, S::Error> {
			Err(serde::ser::Error::custom("fixture serialization failure"))
		}
	}
	let (_server, transport, peer, _keys, calls) = fixture("ok").await;
	let looked_up = AtomicBool::new(false);
	let result = transport
		.transaction::<Value>(
			async {
				looked_up.store(true, Ordering::SeqCst);
				Ok(peer)
			},
			"POST",
			"/rpc",
			Some(&Unserializable),
		)
		.await;
	assert!(
		matches!(result, Err(Error::Json(error)) if error.to_string() == "fixture serialization failure")
	);
	assert!(!looked_up.load(Ordering::SeqCst));
	assert!(calls.lock().unwrap().is_empty());
}
