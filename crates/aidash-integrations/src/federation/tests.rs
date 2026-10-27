use super::*;
use aidash_application::{federation::key::DeploymentKey, provider_access::KeylessAccess};
use aidash_capability::{
	InMemorySigner, PublicKeys, SignError,
	federation::{FederationClaims, RequestBinding},
};
use aidash_domain::{
	keyless::PermittedOrigins,
	nodes::{DeploymentKeyId, NodeAddress, registration::PinnedKey},
	tenancy::TenantName,
};
use axum::{
	Json, Router,
	body::Bytes,
	extract::Path,
	http::{HeaderMap, StatusCode},
	response::{IntoResponse, Response},
	routing::{get, post},
};
use rstest::rstest;
use serde_json::json;
use std::collections::BTreeMap;

/// The sending Deployment's key.
pub(crate) struct TestKey {
	signer: InMemorySigner,
	id: DeploymentKeyId,
	pem: String,
}
impl TestKey {
	pub(crate) fn new() -> Arc<Self> {
		let signer = InMemorySigner::new("unused".into(), [9; 32]);
		let id = DeploymentKeyId::of_public_key(signer.public_key().as_bytes());
		Arc::new(Self {
			pem: signer.public_key_pem(),
			signer,
			id,
		})
	}
}
#[async_trait]
impl DeploymentKey for TestKey {
	fn key_id(&self) -> &DeploymentKeyId {
		&self.id
	}
	fn public_key_pem(&self) -> &str {
		&self.pem
	}
	async fn sign(&self, input: &[u8]) -> std::result::Result<[u8; 64], SignError> {
		use aidash_capability::TokenSigner;
		self.signer.sign(input).await
	}
}

pub(crate) struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
	fn drop(&mut self) {
		self.0.abort();
	}
}
pub(crate) async fn serve(app: Router) -> (Server, String) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let origin = format!("http://{}", listener.local_addr().unwrap());
	(
		Server(tokio::spawn(async move {
			axum::serve(listener, app).await.unwrap()
		})),
		origin,
	)
}

/// A transport that signs with `key` and may reach `origin`.
pub(crate) fn transport(key: &Arc<TestKey>, origin: &str) -> HttpTransport {
	HttpTransport {
		endpoints: endpoint::Endpoints::new(KeylessAccess::permitting(
			PermittedOrigins::parse([origin]).unwrap(),
		))
		.unwrap(),
		deployment: DeploymentId::parse("aidash://local").unwrap(),
		egress: Egress {
			enabled: true,
			key: Some(key.clone()),
		},
	}
}

/// The sending Node's route to a Node of another Deployment at `endpoint`.
pub(crate) fn route(endpoint: &str, subject: Option<&str>) -> Route {
	Route {
		tenant: TenantName::new("tenant").unwrap(),
		sender: NodeAddress::parse("aidash://local/calm-heron-ab12cd34").unwrap(),
		subject: subject.map(str::to_owned),
		target: NodeAddress::parse("aidash://remote/quiet-otter-cd34ef56").unwrap(),
		counterpart: Counterpart::OtherDeployment {
			endpoint: endpoint.into(),
			key: PinnedKey {
				key_id: DeploymentKeyId::parse(&"a".repeat(43)).unwrap(),
				public_key_pem: "pem".into(),
			},
		},
		atomic_transactions: true,
	}
}

/// What the receiver does with a request: verify its token against the
/// request it carries, as `accept` does.
pub(crate) fn verified(
	key: &TestKey,
	headers: &HeaderMap,
	method: &str,
	path: &str,
	body: &[u8],
) -> FederationClaims {
	let token = headers["authorization"]
		.to_str()
		.unwrap()
		.strip_prefix("Bearer ")
		.unwrap();
	let keys = PublicKeys::from_pems(BTreeMap::from([(
		key.id.as_str().to_owned(),
		key.pem.clone(),
	)]))
	.unwrap();
	FederationClaims::verify(
		token,
		&keys,
		chrono::Utc::now().timestamp() as u64,
		&RequestBinding {
			protocol: CURRENT,
			method,
			path,
			body,
		},
	)
	.unwrap()
}

async fn fixture() -> (Server, HttpTransport, Route, Arc<TestKey>) {
	let app = Router::new()
		.route(
			"/federation/v0.1/reply/{status}",
			post(
				|Path(status): Path<u16>, headers: HeaderMap, body: Bytes| async move {
					let body: Value = serde_json::from_slice(&body).unwrap();
					let mut response = (
						StatusCode::from_u16(status).unwrap(),
						Json(json!({
							"headers": {
								"authorization": headers["authorization"].to_str().unwrap(),
								"protocol": headers["x-aidash-protocol"].to_str().unwrap(),
							},
							"body": body,
						})),
					)
						.into_response();
					if status == 503 {
						response
							.headers_mut()
							.insert("x-aidash-transaction-pending", "1".parse().unwrap());
					}
					if status == 409 {
						response
							.headers_mut()
							.insert("x-aidash-run-message-pending", "1".parse().unwrap());
					}
					for header in ["x-aidash-semantic-reason", "x-aidash-semantic-cause"] {
						if let Some(code) = body[header].as_str() {
							response.headers_mut().insert(header, code.parse().unwrap());
						}
					}
					response
				},
			),
		)
		.route(
			"/federation/v0.1/scoped/previews/retain",
			post(|| async {
				(
					StatusCode::CONFLICT,
					Json(json!({"error":"FILE_VERSION_QUOTA_EXCEEDED: remote hold refused"})),
				)
			}),
		)
		.route(
			"/federation/v0.1/oversize",
			get(|| async { Response::new(axum::body::Body::from(vec![b' '; 4_194_305])) }),
		);
	let (server, origin) = serve(app).await;
	let key = TestKey::new();
	(
		server,
		transport(&key, &origin),
		route(&origin, Some("alice")),
		key,
	)
}

#[rstest]
#[tokio::test]
async fn each_request_carries_a_token_minted_for_it() {
	// Arrange
	let (_server, client, route, key) = fixture().await;
	// Act
	let reply = client
		.request(&route, "POST", "/reply/200", Some(&json!({"revision":42})))
		.await
		.unwrap();
	// Assert
	assert_eq!(reply.body["headers"]["protocol"], "0.4");
	assert_eq!(reply.body["body"], json!({"revision":42}));
	let mut headers = HeaderMap::new();
	headers.insert(
		"authorization",
		reply.body["headers"]["authorization"]
			.as_str()
			.unwrap()
			.parse()
			.unwrap(),
	);
	let claims = verified(&key, &headers, "POST", "/reply/200", br#"{"revision":42}"#);
	assert_eq!(claims.iss, "aidash://local");
	assert_eq!(claims.sub, "aidash://local/calm-heron-ab12cd34");
	assert_eq!(claims.aud, "aidash://remote/quiet-otter-cd34ef56");
	assert_eq!(claims.subject, "alice");
	assert_eq!(claims.kid, key.id.as_str());
}

#[rstest]
#[tokio::test]
async fn a_request_acting_as_the_node_names_no_subject() {
	let (_server, client, mut route, key) = fixture().await;
	route.subject = None;
	let reply = client
		.request(&route, "POST", "/reply/200", Some(&json!({})))
		.await
		.unwrap();
	let mut headers = HeaderMap::new();
	headers.insert(
		"authorization",
		reply.body["headers"]["authorization"]
			.as_str()
			.unwrap()
			.parse()
			.unwrap(),
	);
	assert_eq!(
		verified(&key, &headers, "POST", "/reply/200", b"{}").subject,
		""
	);
}

#[rstest]
#[tokio::test]
async fn a_token_does_not_verify_for_another_request() {
	let (_server, client, route, key) = fixture().await;
	let reply = client
		.request(&route, "POST", "/reply/200", Some(&json!({"revision":42})))
		.await
		.unwrap();
	let token = reply.body["headers"]["authorization"]
		.as_str()
		.unwrap()
		.strip_prefix("Bearer ")
		.unwrap()
		.to_owned();
	let keys = PublicKeys::from_pems(BTreeMap::from([(
		key.id.as_str().to_owned(),
		key.pem.clone(),
	)]))
	.unwrap();
	let now = chrono::Utc::now().timestamp() as u64;
	for (method, path, body) in [
		("GET", "/reply/200", &br#"{"revision":42}"#[..]),
		("POST", "/reply/201", br#"{"revision":42}"#),
		("POST", "/reply/200", br#"{"revision":43}"#),
	] {
		assert!(
			FederationClaims::verify(
				&token,
				&keys,
				now,
				&RequestBinding {
					protocol: CURRENT,
					method,
					path,
					body
				}
			)
			.is_err(),
			"{method} {path}"
		);
	}
}

#[rstest]
#[tokio::test]
async fn nothing_is_sent_while_federation_is_off_or_keyless() {
	let (_server, mut client, route, _key) = fixture().await;
	client.egress.enabled = false;
	assert!(matches!(
		client
			.send(&route, "POST", "/reply/200", Some(&json!({})))
			.await,
		Err(Error::FederationDisabled)
	));
	client.egress = Egress {
		enabled: true,
		key: None,
	};
	assert!(matches!(
		client
			.send(&route, "POST", "/reply/200", Some(&json!({})))
			.await,
		Err(Error::FederationDisabled)
	));
}

#[rstest]
#[tokio::test]
async fn a_node_of_this_deployment_is_never_reached_over_http() {
	let (_server, client, mut route, _key) = fixture().await;
	route.counterpart = Counterpart::SameDeployment;
	assert!(matches!(
		client.send(&route, "GET", "/observe", None).await,
		Err(Error::Invalid(_))
	));
}

#[rstest]
#[tokio::test]
async fn a_private_endpoint_the_deployment_does_not_permit_receives_nothing() {
	let (_server, client, mut route, _key) = fixture().await;
	let Counterpart::OtherDeployment { endpoint, .. } = &mut route.counterpart else {
		unreachable!()
	};
	*endpoint = "http://127.0.0.1:1".into();
	assert!(matches!(
		client.send(&route, "GET", "/observe", None).await,
		Err(Error::Forbidden)
	));
}

#[rstest]
#[tokio::test]
async fn a_plaintext_network_endpoint_cannot_receive_tokens() {
	let (_server, client, mut route, _key) = fixture().await;
	let Counterpart::OtherDeployment { endpoint, .. } = &mut route.counterpart else {
		unreachable!()
	};
	*endpoint = "http://peer.example.test".into();
	assert!(matches!(
		client.send(&route, "GET", "/observe", None).await,
		Err(Error::Invalid(_))
	));
}

#[rstest]
#[case::specific(
	Some("durable_memory_budget"),
	Some("context_budget"),
	Some(aidash_domain::semantic::Failure::DurableMemoryBudget)
)]
#[case::legacy(
	None,
	Some("context_budget"),
	Some(aidash_domain::semantic::Failure::ContextBudget)
)]
#[case::future(
	Some("future_budget_cause"),
	Some("context_budget"),
	Some(aidash_domain::semantic::Failure::ContextBudget)
)]
#[case::unknown(
	None,
	Some("future_failure"),
	Some(aidash_domain::semantic::Failure::ContextBudget)
)]
#[case::ordinary(None, None, None)]
#[tokio::test]
async fn authority_transport_preserves_specific_and_compatible_semantic_causes(
	#[case] cause: Option<&str>,
	#[case] reason: Option<&str>,
	#[case] expected: Option<aidash_domain::semantic::Failure>,
) {
	use aidash_application::ports::federation::authority::Transport;
	let (_server, client, peer, _keys) = fixture().await;
	let reply = Transport::request(
		&client,
		&peer,
		"/reply/409",
		&json!({
			"x-aidash-semantic-reason":reason,"x-aidash-semantic-cause":cause
		}),
	)
	.await
	.unwrap();
	assert_eq!(reply.status, 409);
	assert_eq!(reply.semantic_reason, expected);
	assert!(reply.body.unwrap().is_empty());
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
	let (_server, client, route, _key) = fixture().await;
	let reply = client
		.request(
			&route,
			"POST",
			&format!("/reply/{status}"),
			Some(&json!({})),
		)
		.await
		.unwrap();
	assert_eq!(reply.status, status);
	assert_eq!(reply.transaction_pending, transaction_pending);
	assert_eq!(reply.run_message_pending, run_message_pending);
	assert_eq!(reply.body.is_null(), status != 400);
}

#[rstest]
#[tokio::test]
async fn remote_preview_conflict_keeps_the_public_reason() {
	let (_server, client, route, _key) = fixture().await;
	let reply = client
		.request(&route, "POST", "/scoped/previews/retain", Some(&json!({})))
		.await
		.unwrap();
	assert_eq!(reply.status, 409);
	assert_eq!(
		reply.body,
		json!({"error":"FILE_VERSION_QUOTA_EXCEEDED: remote hold refused"})
	);
}

#[rstest]
#[tokio::test]
async fn oversized_peer_success_is_rejected_before_json_decoding() {
	let (_server, client, route, _key) = fixture().await;
	let result = client.request(&route, "GET", "/oversize", None).await;
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
	let (_server, origin) = serve(app).await;
	let key = TestKey::new();
	let client = transport(&key, &origin);
	let result = client
		.request(&route(&origin, None), "POST", path, Some(&json!({})))
		.await;
	assert_eq!(observed.load(Ordering::SeqCst), 1);
	if leaf_timeout {
		assert!(matches!(result, Err(Error::External(_))));
	} else {
		assert_eq!(result.unwrap().body, json!({"completed":true}));
	}
}
