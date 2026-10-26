//! Tenant Secret role acceptance. The public-address check is replaced only
//! here, through the test-only origin override; production always keeps
//! `PublicDns` and `public_client`.
use super::json_body;
use crate::tenant_secret::{
	SecretCallAudit, SecretCallSink, TenantSecretBroker, TenantSecretConfig,
};
use aidash_capability::{
	Claims, InMemorySigner, KeyMaterialError, Operation, PublicKeys, TokenSubject, mint,
	tenant_secret::{
		Attach, Destination, SealedSecretSource, SecretClaims, SecretSubject, mint_secret, sealed,
	},
};
use async_trait::async_trait;
use axum::{
	Json, Router,
	body::{Body, Bytes, to_bytes},
	extract::{Request, State},
	http::{HeaderMap, StatusCode, header},
	response::{IntoResponse, Response},
	routing::any,
};
use futures_util::StreamExt;
use secrecy::ExposeSecret;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
	collections::BTreeMap,
	sync::{
		Arc, Mutex,
		atomic::{AtomicBool, AtomicUsize, Ordering},
	},
	time::{Duration, SystemTime, UNIX_EPOCH},
};
use tower::ServiceExt;
use zeroize::Zeroizing;

const SECRET_CANARY: &str = "canary-tenant-secret-value-never-logged";
const SERVER_KID: &str = "projects/app/locations/us-central1/keyRings/broker/cryptoKeys/tenant-secret/cryptoKeyVersions/1";
const WORKER_KID: &str =
	"projects/app/locations/us-central1/keyRings/broker/cryptoKeys/capability/cryptoKeyVersions/1";
const OTHER_KID: &str =
	"projects/app/locations/us-central1/keyRings/broker/cryptoKeys/other/cryptoKeyVersions/1";
const AUDIENCE: &str = "env-tsec";
const HOST: &str = "upstream.test";

#[derive(Default)]
struct SealedSource {
	reads: AtomicUsize,
	unavailable: AtomicBool,
	store_unavailable: AtomicBool,
	payload: Mutex<Vec<u8>>,
}
impl SealedSource {
	fn payload(&self, bytes: Vec<u8>) {
		*self.payload.lock().unwrap() = bytes;
	}
}
#[async_trait]
impl SealedSecretSource for SealedSource {
	async fn access(&self, _: &str, _: &str) -> Result<Zeroizing<Vec<u8>>, KeyMaterialError> {
		self.reads.fetch_add(1, Ordering::SeqCst);
		if self.store_unavailable.load(Ordering::SeqCst) {
			return Err(KeyMaterialError::StoreUnavailable);
		}
		if self.unavailable.load(Ordering::SeqCst) {
			return Err(KeyMaterialError::Unavailable);
		}
		Ok(Zeroizing::new(self.payload.lock().unwrap().clone()))
	}
}

#[derive(Default)]
struct SecretLogs(Mutex<Vec<String>>);
impl SecretCallSink for SecretLogs {
	fn record(&self, audit: &SecretCallAudit) {
		self.0
			.lock()
			.unwrap()
			.push(serde_json::to_string(audit).unwrap());
	}
}

#[derive(Default)]
struct Seen {
	method: String,
	path: String,
	query: Option<String>,
	headers: HeaderMap,
	body: Vec<u8>,
}
#[derive(Default)]
struct UpstreamState {
	calls: AtomicUsize,
	seen: Mutex<Vec<Seen>>,
	finish: tokio::sync::Notify,
	completed: AtomicBool,
}
async fn handle(State(state): State<Arc<UpstreamState>>, request: Request) -> Response {
	state.calls.fetch_add(1, Ordering::SeqCst);
	let method = request.method().as_str().to_owned();
	let path = request.uri().path().to_owned();
	let query = request.uri().query().map(str::to_owned);
	let headers = request.headers().clone();
	let body = to_bytes(request.into_body(), 64 * 1024 * 1024)
		.await
		.unwrap()
		.to_vec();
	state.seen.lock().unwrap().push(Seen {
		method: method.clone(),
		path: path.clone(),
		query,
		headers: headers.clone(),
		body: body.clone(),
	});
	match path.as_str() {
		"/api/sse" => {
			let state = state.clone();
			let stream = async_stream::stream! {
				yield Ok::<_, std::io::Error>(Bytes::from_static(
					b"data: {\"early\":true}\n\n",
				));
				state.finish.notified().await;
				state.completed.store(true, Ordering::SeqCst);
				yield Ok(Bytes::from_static(b"data: {\"late\":true}\n\n"));
			};
			Response::builder()
				.header(header::CONTENT_TYPE, "text/event-stream")
				.header("mcp-session-id", "server-session")
				.header("last-event-id", "server-event")
				.body(Body::from_stream(stream))
				.unwrap()
		}
		"/api/redirect" => Response::builder()
			.status(StatusCode::FOUND)
			.header(header::LOCATION, "https://elsewhere.test/next")
			.body(Body::empty())
			.unwrap(),
		"/api/large" => Response::new(Body::from(vec![b'x'; 1000])),
		"/api/teapot" => (StatusCode::IM_A_TEAPOT, "teapot").into_response(),
		"/api/slow" => {
			tokio::time::sleep(Duration::from_millis(500)).await;
			Json(json!({"slow":true})).into_response()
		}
		_ => Json(json!({
			"method": method,
			"path": path,
			"body": String::from_utf8_lossy(&body),
		}))
		.into_response(),
	}
}

struct SecretFixture {
	broker: Arc<TenantSecretBroker>,
	server_signer: InMemorySigner,
	other_signer: InMemorySigner,
	source: Arc<SealedSource>,
	logs: Arc<SecretLogs>,
	upstream: Arc<UpstreamState>,
	server: tokio::task::JoinHandle<()>,
}
impl Drop for SecretFixture {
	fn drop(&mut self) {
		self.server.abort();
	}
}
impl SecretFixture {
	async fn new() -> Self {
		Self::build(|_| {}, None).await
	}
	async fn with_config(configure: impl FnOnce(&mut TenantSecretConfig)) -> Self {
		Self::build(configure, None).await
	}
	/// Keep the production public-address check. The local upstream is never
	/// reachable, so a non-public answer is refused before any connection.
	async fn without_origin_override() -> Self {
		Self::build(|_| {}, Some(None)).await
	}
	async fn with_dead_origin() -> Self {
		Self::build(|_| {}, Some(Some("http://127.0.0.1:1".into()))).await
	}
	async fn build(
		configure: impl FnOnce(&mut TenantSecretConfig),
		origin: Option<Option<String>>,
	) -> Self {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let base = format!("http://{}", listener.local_addr().unwrap());
		let upstream = Arc::new(UpstreamState::default());
		let router = Router::new()
			.fallback(any(handle))
			.with_state(upstream.clone());
		let server = tokio::spawn(async move {
			axum::serve(listener, router).await.unwrap();
		});
		let server_signer = InMemorySigner::new(SERVER_KID.into(), [7; 32]);
		let worker_signer = InMemorySigner::new(WORKER_KID.into(), [9; 32]);
		let other_signer = InMemorySigner::new(OTHER_KID.into(), [11; 32]);
		let mut keys = PublicKeys::default();
		keys.insert(SERVER_KID.into(), server_signer.public_key());
		keys.insert(WORKER_KID.into(), worker_signer.public_key());
		let source = Arc::new(SealedSource::default());
		source.payload(
			sealed::seal(&[HOST.into()], SECRET_CANARY)
				.unwrap()
				.to_vec(),
		);
		let logs = Arc::new(SecretLogs::default());
		let mut config = TenantSecretConfig {
			byok_project_number: "1234".into(),
			secret_prefix: "aidash-test-tsec-".into(),
			audience: AUDIENCE.into(),
			issuers: BTreeMap::from([
				(SERVER_KID.into(), "aidash-server".into()),
				(WORKER_KID.into(), "aidash-worker".into()),
			]),
			requests_per_second: 100.0,
			burst: 100,
			timeout: Duration::from_secs(5),
		};
		configure(&mut config);
		let mut broker =
			TenantSecretBroker::new(config, keys, source.clone(), logs.clone()).unwrap();
		match origin {
			Some(Some(origin)) => broker.test_origin(origin),
			Some(None) => {}
			None => broker.test_origin(base.clone()),
		}
		Self {
			broker: Arc::new(broker),
			server_signer,
			other_signer,
			source,
			logs,
			upstream,
			server,
		}
	}
	fn claims(&self) -> SecretClaims {
		let now = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap()
			.as_secs();
		SecretClaims {
			iss: "aidash-server".into(),
			aud: AUDIENCE.into(),
			iat: now,
			exp: now + 60,
			jti: uuid::Uuid::new_v4(),
			kid: SERVER_KID.into(),
			tenant: "tenant-a".into(),
			secret: uuid::Uuid::from_u128(42),
			version: "1".into(),
			destination: Destination {
				origin: format!("https://{HOST}"),
				path_prefix: "/api/".into(),
			},
			methods: vec![
				"GET".into(),
				"POST".into(),
				"PUT".into(),
				"PATCH".into(),
				"DELETE".into(),
			],
			attach: Attach::Bearer,
			sub: SecretSubject::Run {
				run: "run-1".into(),
				call: uuid::Uuid::new_v4(),
			},
			max_response_bytes: None,
		}
	}
	async fn request(
		&self,
		claims: &SecretClaims,
		method: &str,
		uri: &str,
		body: Body,
	) -> Response {
		let token = mint_secret(claims, &self.server_signer).await.unwrap();
		self.request_token(token.expose_secret(), method, uri, body)
			.await
	}
	async fn request_signed(
		&self,
		claims: &SecretClaims,
		signer: &InMemorySigner,
		method: &str,
		uri: &str,
	) -> Response {
		let token = mint_secret(claims, signer).await.unwrap();
		self.request_token(token.expose_secret(), method, uri, Body::empty())
			.await
	}
	async fn request_token(&self, token: &str, method: &str, uri: &str, body: Body) -> Response {
		self.broker
			.clone()
			.router()
			.oneshot(
				Request::builder()
					.method(method)
					.uri(uri)
					.header("x-aidash-secret-call", token)
					.header(header::AUTHORIZATION, "Bearer inbound-must-be-stripped")
					.header(header::COOKIE, "session=inbound")
					.header("x-forwarded-for", "203.0.113.1")
					.header("x-aidash-other", "strip-me")
					.header("x-hop", "strip-me")
					.header(header::CONNECTION, "keep-alive, x-hop")
					.header(header::HOST, "attacker.invalid")
					.header(header::CONTENT_TYPE, "application/json")
					.header(header::ACCEPT, "text/event-stream")
					.header("x-api-key", "inbound-must-be-replaced")
					.header("mcp-session-id", "client-session")
					.header("last-event-id", "client-event")
					.body(body)
					.unwrap(),
			)
			.await
			.unwrap()
	}
	async fn request_without_token(&self, uri: &str) -> Response {
		self.broker
			.clone()
			.router()
			.oneshot(
				Request::builder()
					.method("GET")
					.uri(uri)
					.body(Body::empty())
					.unwrap(),
			)
			.await
			.unwrap()
	}
}
async fn assert_error(response: Response, status: u16, code: &str, jti: Option<uuid::Uuid>) {
	assert_eq!(response.status(), status, "{code}");
	assert_eq!(response.headers()["x-aidash-broker-error"], code);
	match jti {
		Some(jti) => assert_eq!(response.headers()["x-aidash-correlation"], jti.to_string()),
		None => assert!(
			response.headers().get("x-aidash-correlation").is_none(),
			"an unverified token must not name a correlation id"
		),
	}
	let body = json_body(response).await;
	assert_eq!(body["error"], code);
	assert_eq!(body.as_object().unwrap().len(), 1);
}
fn audit(f: &SecretFixture, index: usize) -> Value {
	let logs = f.logs.0.lock().unwrap();
	serde_json::from_str(&logs[index]).unwrap()
}

#[tokio::test]
async fn get_and_post_forward_streamed_bodies_and_attach_the_value() {
	let f = SecretFixture::new().await;
	let get = f
		.request(&f.claims(), "GET", "/v1/forward/echo?x=1", Body::empty())
		.await;
	assert_eq!(get.status(), 200);
	let get_body = json_body(get).await;
	assert_eq!(get_body["method"], "GET");
	assert_eq!(get_body["path"], "/api/echo");

	let post_body = Body::from_stream(async_stream::stream! {
		yield Ok::<_, std::io::Error>(Bytes::from_static(b"hello "));
		yield Ok(Bytes::from_static(b"world"));
	});
	let post = f
		.request(&f.claims(), "POST", "/v1/forward/echo", post_body)
		.await;
	assert_eq!(post.status(), 200);
	assert_eq!(json_body(post).await["body"], "hello world");

	let seen = f.upstream.seen.lock().unwrap();
	assert_eq!(seen.len(), 2);
	assert_eq!(seen[0].method, "GET");
	assert_eq!(seen[1].method, "POST");
	assert_eq!(seen[0].path, "/api/echo");
	assert_eq!(seen[0].query.as_deref(), Some("x=1"));
	assert_eq!(
		seen[0].headers[header::AUTHORIZATION].to_str().unwrap(),
		format!("Bearer {SECRET_CANARY}")
	);
	assert_eq!(seen[0].headers["mcp-session-id"], "client-session");
	assert_eq!(seen[0].headers["last-event-id"], "client-event");
	assert_eq!(seen[0].headers[header::ACCEPT], "text/event-stream");
	for name in ["cookie", "x-forwarded-for", "x-aidash-other", "x-hop"] {
		assert!(!seen[0].headers.contains_key(name), "{name}");
	}
	assert_ne!(seen[0].headers[header::HOST], "attacker.invalid");
	assert_eq!(seen[1].body, b"hello world");
	drop(seen);

	// The same Tenant Secret and version is read once and then served from the
	// short cache.
	assert_eq!(f.source.reads.load(Ordering::SeqCst), 1);
	let entry = audit(&f, 0);
	assert_eq!(entry["tenant"], "tenant-a");
	assert_eq!(entry["secret"], uuid::Uuid::from_u128(42).to_string());
	assert_eq!(entry["version"], "1");
	assert_eq!(entry["host"], HOST);
	assert_eq!(entry["method"], "GET");
	assert_eq!(entry["status"], 200);
	assert_eq!(entry["bytes_in"], 0);
	assert_eq!(entry["subject"], "run");
	let digest: String = Sha256::digest(b"/api/echo")
		.iter()
		.map(|byte| format!("{byte:02x}"))
		.collect();
	assert_eq!(entry["path_sha256"], digest[..16]);
	let mut keys: Vec<_> = entry
		.as_object()
		.unwrap()
		.keys()
		.map(String::as_str)
		.collect();
	keys.sort_unstable();
	assert_eq!(
		keys,
		[
			"bytes_in",
			"bytes_out",
			"duration_ms",
			"host",
			"jti",
			"method",
			"path_sha256",
			"secret",
			"status",
			"subject",
			"tenant",
			"version",
		]
	);
	assert_eq!(audit(&f, 1)["bytes_in"], 11);
	let logs = f.logs.0.lock().unwrap();
	assert!(
		logs.iter()
			.all(|line| !line.contains(SECRET_CANARY) && !line.contains("inbound"))
	);
}

#[tokio::test]
async fn sse_streams_early_and_passes_session_headers() {
	let f = SecretFixture::new().await;
	let response = f
		.request(&f.claims(), "GET", "/v1/forward/sse", Body::empty())
		.await;
	assert_eq!(response.status(), 200);
	assert_eq!(
		response.headers()[header::CONTENT_TYPE],
		"text/event-stream"
	);
	assert_eq!(response.headers()["mcp-session-id"], "server-session");
	assert_eq!(response.headers()["last-event-id"], "server-event");
	let mut stream = response.into_body().into_data_stream();
	let chunk = tokio::time::timeout(Duration::from_secs(1), stream.next())
		.await
		.unwrap()
		.unwrap()
		.unwrap();
	assert!(std::str::from_utf8(&chunk).unwrap().contains("early"));
	assert!(!f.upstream.completed.load(Ordering::SeqCst));
	f.upstream.finish.notify_one();
	let mut rest = Vec::new();
	while let Some(chunk) = stream.next().await {
		rest.extend_from_slice(&chunk.unwrap());
	}
	assert!(std::str::from_utf8(&rest).unwrap().contains("late"));
	assert!(f.upstream.completed.load(Ordering::SeqCst));
	let seen = f.upstream.seen.lock().unwrap();
	assert_eq!(seen[0].headers["mcp-session-id"], "client-session");
	assert_eq!(seen[0].headers["last-event-id"], "client-event");
	assert_eq!(seen[0].headers[header::ACCEPT], "text/event-stream");
}

#[tokio::test]
async fn every_broker_owned_error_has_its_contract_shape() {
	let f = SecretFixture::new().await;
	assert_error(
		f.request_without_token("/v1/forward/echo").await,
		401,
		"broker_rejected",
		None,
	)
	.await;
	assert_error(
		f.request_token("not-a-token", "GET", "/v1/forward/echo", Body::empty())
			.await,
		401,
		"broker_rejected",
		None,
	)
	.await;

	let mut wrong_audience = f.claims();
	wrong_audience.aud = "other".into();
	assert_error(
		f.request(&wrong_audience, "GET", "/v1/forward/echo", Body::empty())
			.await,
		401,
		"broker_rejected",
		None,
	)
	.await;

	let mut expired = f.claims();
	let now = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap()
		.as_secs();
	expired.iat = now - 61;
	expired.exp = now - 1;
	assert_error(
		f.request(&expired, "GET", "/v1/forward/echo", Body::empty())
			.await,
		401,
		"broker_rejected",
		None,
	)
	.await;

	let mut unknown_kid = f.claims();
	unknown_kid.kid = OTHER_KID.into();
	assert_error(
		f.request_signed(&unknown_kid, &f.other_signer, "GET", "/v1/forward/echo")
			.await,
		401,
		"broker_rejected",
		None,
	)
	.await;

	// A server key may speak only for the server issuer.
	let mut worker_issuer = f.claims();
	worker_issuer.iss = "aidash-worker".into();
	assert_error(
		f.request(&worker_issuer, "GET", "/v1/forward/echo", Body::empty())
			.await,
		401,
		"broker_rejected",
		None,
	)
	.await;

	let mut method = f.claims();
	method.methods = vec!["GET".into()];
	assert_error(
		f.request(&method, "POST", "/v1/forward/echo", Body::empty())
			.await,
		403,
		"secret_destination_refused",
		Some(method.jti),
	)
	.await;

	let mut refused = f.claims();
	refused.destination.origin = "https://other.test".into();
	assert_error(
		f.request(&refused, "GET", "/v1/forward/echo", Body::empty())
			.await,
		403,
		"secret_destination_refused",
		Some(refused.jti),
	)
	.await;
	assert_eq!(f.upstream.calls.load(Ordering::SeqCst), 0);

	// Different versions keep these failures out of the short sealed cache.
	let mut plain = f.claims();
	plain.version = "2".into();
	f.source.payload(b"plain-secret".to_vec());
	assert_error(
		f.request(&plain, "GET", "/v1/forward/echo", Body::empty())
			.await,
		404,
		"secret_unavailable",
		Some(plain.jti),
	)
	.await;

	let mut old = f.claims();
	old.version = "3".into();
	f.source
		.payload(br#"{"v":0,"hosts":["upstream.test"],"value":"old"}"#.to_vec());
	assert_error(
		f.request(&old, "GET", "/v1/forward/echo", Body::empty())
			.await,
		404,
		"secret_unavailable",
		Some(old.jti),
	)
	.await;

	let mut missing = f.claims();
	missing.version = "4".into();
	f.source.unavailable.store(true, Ordering::SeqCst);
	assert_error(
		f.request(&missing, "GET", "/v1/forward/echo", Body::empty())
			.await,
		404,
		"secret_unavailable",
		Some(missing.jti),
	)
	.await;
	f.source.unavailable.store(false, Ordering::SeqCst);

	let mut outage = f.claims();
	outage.version = "5".into();
	f.source.store_unavailable.store(true, Ordering::SeqCst);
	assert_error(
		f.request(&outage, "GET", "/v1/forward/echo", Body::empty())
			.await,
		502,
		"upstream_unreachable",
		Some(outage.jti),
	)
	.await;
}

#[tokio::test]
async fn method_not_allowed_and_path_traversal_never_reach_the_secret() {
	let f = SecretFixture::new().await;
	let mut claims = f.claims();
	claims.methods = vec!["GET".into()];
	assert_error(
		f.request(&claims, "POST", "/v1/forward/echo", Body::empty())
			.await,
		403,
		"secret_destination_refused",
		Some(claims.jti),
	)
	.await;
	assert_eq!(f.source.reads.load(Ordering::SeqCst), 0);

	for uri in [
		"/v1/forward/../secret",
		"/v1/forward/%2e%2e/secret",
		"/v1/forward/%2E%2E/secret",
		"/v1/forward/..%2fsecret",
		"/v1/forward/%2e%2e%2fsecret",
		"/v1/forward/a/%2e%2e/b",
		"/v1/forward//secret",
		"/v1/forward/foo/..",
		"/v1/forward/foo%5c..%5csecret",
		"/v1/forward/foo\\bar",
	] {
		let claims = f.claims();
		assert_error(
			f.request(&claims, "GET", uri, Body::empty()).await,
			403,
			"secret_destination_refused",
			Some(claims.jti),
		)
		.await;
	}
	assert_eq!(f.source.reads.load(Ordering::SeqCst), 0);
	assert_eq!(f.upstream.calls.load(Ordering::SeqCst), 0);
	assert!(
		f.upstream
			.seen
			.lock()
			.unwrap()
			.iter()
			.all(|seen| !seen.path.contains("secret"))
	);
}

#[tokio::test]
async fn sealed_hosts_and_the_public_address_check_are_enforced() {
	let f = SecretFixture::new().await;
	let mut refused = f.claims();
	refused.destination.origin = "https://other.test".into();
	assert_error(
		f.request(&refused, "GET", "/v1/forward/echo", Body::empty())
			.await,
		403,
		"secret_destination_refused",
		Some(refused.jti),
	)
	.await;

	let public = SecretFixture::without_origin_override().await;
	public.source.payload(
		sealed::seal(&["localhost".into()], SECRET_CANARY)
			.unwrap()
			.to_vec(),
	);
	let mut loopback = public.claims();
	loopback.destination.origin = "https://localhost:9".into();
	assert_error(
		public
			.request(&loopback, "GET", "/v1/forward/echo", Body::empty())
			.await,
		403,
		"secret_destination_refused",
		Some(loopback.jti),
	)
	.await;
	assert_eq!(public.upstream.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unsealed_old_and_unavailable_payloads_are_404() {
	let f = SecretFixture::new().await;
	for payload in [
		br#"{"v":1,"hosts":["upstream.test"]}"#.to_vec(),
		br#"{"v":2,"hosts":["upstream.test"],"value":"x"}"#.to_vec(),
		br#"{"v":1,"hosts":[],"value":"x"}"#.to_vec(),
		br#"{"v":1,"hosts":["upstream.test"],"value":"x","extra":true}"#.to_vec(),
		br#"{"hosts":["upstream.test"],"value":"x"}"#.to_vec(),
	] {
		f.source.payload(payload.clone());
		let claims = f.claims();
		assert_error(
			f.request(&claims, "GET", "/v1/forward/echo", Body::empty())
				.await,
			404,
			"secret_unavailable",
			Some(claims.jti),
		)
		.await;
	}
	assert_eq!(f.upstream.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn redirects_are_returned_unchanged_and_not_followed() {
	let f = SecretFixture::new().await;
	let response = f
		.request(&f.claims(), "GET", "/v1/forward/redirect", Body::empty())
		.await;
	assert_eq!(response.status(), 302);
	assert_eq!(
		response.headers()[header::LOCATION],
		"https://elsewhere.test/next"
	);
	assert_eq!(f.upstream.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn attach_header_replaces_the_inbound_value_and_bearer_stays_sensitive() {
	let f = SecretFixture::new().await;
	let mut header = f.claims();
	header.attach = Attach::Header {
		name: "x-api-key".into(),
	};
	let response = f
		.request(&header, "GET", "/v1/forward/echo", Body::empty())
		.await;
	assert_eq!(response.status(), 200);
	json_body(response).await;
	let seen = f.upstream.seen.lock().unwrap();
	assert_eq!(seen[0].headers["x-api-key"], SECRET_CANARY);
	assert!(!seen[0].headers.contains_key(header::AUTHORIZATION));
}

#[tokio::test]
async fn max_response_bytes_cuts_the_stream() {
	let f = SecretFixture::new().await;
	let mut claims = f.claims();
	claims.max_response_bytes = Some(100);
	let response = f
		.request(&claims, "GET", "/v1/forward/large", Body::empty())
		.await;
	assert_eq!(response.status(), 200);
	let body = to_bytes(response.into_body(), 4096).await.unwrap();
	assert_eq!(body.len(), 100);
	let entry = audit(&f, 0);
	assert_eq!(entry["status"], 200);
	assert_eq!(entry["bytes_out"], 100);
}

#[tokio::test]
async fn rate_limiting_is_per_tenant_and_secret() {
	let f = SecretFixture::with_config(|config| {
		config.requests_per_second = 0.001;
		config.burst = 1;
	})
	.await;
	let first = f
		.request(&f.claims(), "GET", "/v1/forward/echo", Body::empty())
		.await;
	assert_eq!(first.status(), 200);
	let mut second = f.claims();
	second.jti = uuid::Uuid::new_v4();
	assert_error(
		f.request(&second, "GET", "/v1/forward/echo", Body::empty())
			.await,
		429,
		"rate_limited",
		Some(second.jti),
	)
	.await;
	let mut other = f.claims();
	other.secret = uuid::Uuid::from_u128(43);
	assert_eq!(
		f.request(&other, "GET", "/v1/forward/echo", Body::empty())
			.await
			.status(),
		200
	);
}

#[tokio::test]
async fn upstream_connect_failure_and_timeout_are_502() {
	let dead = SecretFixture::with_dead_origin().await;
	let dead_claims = dead.claims();
	assert_error(
		dead.request(&dead_claims, "GET", "/v1/forward/echo", Body::empty())
			.await,
		502,
		"upstream_unreachable",
		Some(dead_claims.jti),
	)
	.await;

	let slow = SecretFixture::with_config(|config| {
		config.timeout = Duration::from_millis(100);
	})
	.await;
	let slow_claims = slow.claims();
	assert_error(
		slow.request(&slow_claims, "GET", "/v1/forward/slow", Body::empty())
			.await,
		502,
		"upstream_unreachable",
		Some(slow_claims.jti),
	)
	.await;
}

#[tokio::test]
async fn provider_tokens_and_server_key_worker_issuers_are_refused() {
	let f = SecretFixture::new().await;
	let now = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap()
		.as_secs();
	let provider = Claims {
		iss: "aidash-server".into(),
		aud: AUDIENCE.into(),
		iat: now,
		exp: now + 60,
		jti: uuid::Uuid::new_v4(),
		kid: SERVER_KID.into(),
		tenant: "tenant-a".into(),
		provider: "openrouter".into(),
		credential: uuid::Uuid::from_u128(1),
		version: "1".into(),
		sub: TokenSubject::Run {
			run: "run-1".into(),
			call: uuid::Uuid::new_v4(),
		},
		ops: vec![Operation::Chat],
		model: "author/model".into(),
		max_output_tokens: 10,
		cache_breakpoints: false,
		hosts: vec![],
	};
	let token = mint(&provider, &f.server_signer).await.unwrap();
	assert_error(
		f.request_token(
			token.expose_secret(),
			"GET",
			"/v1/forward/echo",
			Body::empty(),
		)
		.await,
		401,
		"broker_rejected",
		None,
	)
	.await;
}

#[tokio::test]
async fn audits_are_recorded_for_rejected_and_accepted_requests() {
	let f = SecretFixture::new().await;
	assert_error(
		f.request_without_token("/v1/forward/echo").await,
		401,
		"broker_rejected",
		None,
	)
	.await;
	let rejected = audit(&f, 0);
	assert_eq!(rejected["status"], 401);
	assert_eq!(rejected["subject"], "unknown");
	assert_eq!(rejected["jti"], uuid::Uuid::nil().to_string());
	assert_eq!(rejected["tenant"], "");

	let mut claims = f.claims();
	claims.destination.origin = "https://other.test".into();
	assert_error(
		f.request(&claims, "GET", "/v1/forward/echo", Body::empty())
			.await,
		403,
		"secret_destination_refused",
		Some(claims.jti),
	)
	.await;
	let refused = audit(&f, 1);
	assert_eq!(refused["status"], 403);
	assert_eq!(refused["jti"], claims.jti.to_string());
	assert_eq!(refused["tenant"], "tenant-a");
	assert_eq!(refused["host"], "other.test");
	assert_eq!(refused["subject"], "run");
}
