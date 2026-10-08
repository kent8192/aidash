use super::*;
use axum::{
	Json, Router,
	extract::Path,
	http::{HeaderMap, StatusCode},
	response::{IntoResponse, Response},
	routing::{get, post},
};
use rstest::rstest;
use serde_json::json;
use std::sync::RwLock;

struct Keys(RwLock<String>);
impl Credentials for Keys {
	fn resolve(&self, reference: &str) -> Result<String> {
		assert_eq!(reference, "FIXTURE_KEY");
		Ok(self.0.read().unwrap().clone())
	}
}
struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
	fn drop(&mut self) {
		self.0.abort();
	}
}
async fn fixture() -> (Server, PeerHttp, Peer, Arc<Keys>) {
	let app=Router::new()
        .route("/.well-known/aidash",get(|| async {Json(json!({"id":"aidash://remote","protocol_version":"0.1"}))}))
        .route("/federation/v0.1/reply/{status}",post(|Path(status):Path<u16>,headers:HeaderMap,Json(body):Json<Value>| async move {
            let mut response=(StatusCode::from_u16(status).unwrap(),Json(json!({"authorization":headers["authorization"].to_str().unwrap(),"node":headers["x-aidash-node"].to_str().unwrap(),"protocol":headers["x-aidash-protocol"].to_str().unwrap(),"body":body}))).into_response();
            if status==503 {response.headers_mut().insert("x-aidash-transaction-pending","1".parse().unwrap());}
            if status==409 {response.headers_mut().insert("x-aidash-run-message-pending","1".parse().unwrap());}
            response
        }))
        .route("/federation/v0.1/oversize",get(|| async {Response::new(axum::body::Body::from(vec![b' ';4_194_305]))}));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = Server(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap()
	}));
	let keys = Arc::new(Keys(RwLock::new("before-rotation".into())));
	let client = PeerHttp {
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
		credential_env: "FIXTURE_KEY".into(),
		protocol_version: "0.1".into(),
		enabled: true,
	};
	(server, client, peer, keys)
}
#[rstest]
#[tokio::test]
async fn federation_headers_payload_and_credential_rotation_are_preserved() {
	// Arrange
	let (_server, client, peer, keys) = fixture().await;
	// Act
	let first = client
		.request(&peer, "POST", "/reply/200", Some(&json!({"revision":42})))
		.await
		.unwrap();
	*keys.0.write().unwrap() = "after-rotation".into();
	let second = client
		.request(&peer, "POST", "/reply/200", Some(&json!({"revision":43})))
		.await
		.unwrap();
	// Assert
	assert_eq!(
		first.body,
		json!({"authorization":"Bearer before-rotation","node":"aidash://local","protocol":"0.1","body":{"revision":42}})
	);
	assert_eq!(second.body["authorization"], "Bearer after-rotation");
	assert_eq!(second.body["body"], json!({"revision":43}));
	assert_eq!(
		client.identity(&peer).await.unwrap(),
		json!({"id":"aidash://remote","protocol_version":"0.1"})
	);
}
#[rstest]
#[case(503, true, false)]
#[case(409, false, true)]
#[case(400, false, false)]
#[tokio::test]
async fn retry_headers_survive_the_transport_boundary(
	#[case] status: u16,
	#[case] transaction_pending: bool,
	#[case] run_message_pending: bool,
) {
	let (_server, client, peer, _keys) = fixture().await;
	let reply = client
		.request(&peer, "POST", &format!("/reply/{status}"), Some(&json!({})))
		.await
		.unwrap();
	assert_eq!(reply.status, status);
	assert_eq!(reply.transaction_pending, transaction_pending);
	assert_eq!(reply.run_message_pending, run_message_pending);
	assert_eq!(reply.body.is_null(), status != 400);
}
#[rstest]
#[tokio::test]
async fn oversized_peer_success_is_rejected_before_json_decoding() {
	let (_server, client, peer, _keys) = fixture().await;
	let result = client.request(&peer, "GET", "/oversize", None).await;
	assert!(
		matches!(result,Err(Error::External(message)) if message == "response exceeds 4194304 bytes")
	);
}

#[rstest]
#[case("/scoped/semantic/query", false)]
#[case("/scoped/execution/admissions/fixture/activate", false)]
#[case("/leaf", true)]
#[tokio::test]
async fn composite_authority_responses_can_complete_after_the_leaf_deadline(
	#[case] path: &str,
	#[case] leaf_timeout: bool,
) {
	use std::sync::atomic::{AtomicUsize, Ordering};
	let entered = Arc::new(AtomicUsize::new(0));
	let observed = entered.clone();
	let app = Router::new().route(
		&format!("/federation/v0.1{path}"),
		post(move || {
			let entered = entered.clone();
			async move {
				entered.fetch_add(1, Ordering::SeqCst);
				tokio::time::sleep(Duration::from_secs(11)).await;
				Json(json!({"completed":true}))
			}
		}),
	);
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let _server = Server(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap();
	}));
	let (_fixture, client, mut peer, _keys) = fixture().await;
	peer.endpoint = endpoint;
	let result = client.request(&peer, "POST", path, Some(&json!({}))).await;
	assert_eq!(observed.load(Ordering::SeqCst), 1);
	if leaf_timeout {
		assert!(matches!(result, Err(Error::External(_))));
	} else {
		assert_eq!(result.unwrap().body, json!({"completed":true}));
	}
}
