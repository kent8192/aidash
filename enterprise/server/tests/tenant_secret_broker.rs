//! The real Tenant Secret broker, the real server-side adapter and the
//! real transports, run together over loopback. The broker's upstream origin is
//! replaced only through the `test-support` feature; the public-address and
//! sealed-host checks still apply to each token's destination.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database/network boundaries directly.

use aidash_application::{
	Error,
	egress::{Admission, Authority, Direction, Meter},
	ports::tools::ToolTransport,
	provider_access::KeylessAccess,
	provider_credentials::Store as SecretStore,
	tenant_secrets::{
		CallSecret, Put, Repository, Resolved, Scope, SecretCall, SecretCallGrant, SecretCalls,
		Service, endpoint_destination,
	},
};
use aidash_broker::tenant_secret::{
	SecretCallAudit, SecretCallSink, TenantSecretBroker, TenantSecretConfig,
};
use aidash_capability::{
	Claims, InMemorySigner, KeyMaterialError, Operation, PublicKeys, TokenSubject, mint,
	tenant_secret::{
		Attach, Destination, SealedSecretSource, SecretClaims, SecretSubject, mint_secret, sealed,
	},
};
use aidash_domain::{
	egress::{Authentication, ExternalService},
	keyless::{Destination as Address, PermittedOrigins},
	tenant_secrets::TenantSecret,
	tool::ToolConfig,
};
use aidash_gcp::capability::{
	secret_calls::SecretCallsAdapter, secret_issuer::SecretIssuerConfiguration,
};
use aidash_integrations::{egress, tools::HttpTools, tools::ToolClients};
use async_trait::async_trait;
use axum::{
	Json, Router,
	body::{Body, to_bytes},
	extract::{Request, State},
	http::{HeaderMap, StatusCode, header},
	response::{IntoResponse, Response},
	routing::any,
};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use std::{
	collections::BTreeMap,
	sync::{
		Arc, Mutex,
		atomic::{AtomicBool, AtomicUsize, Ordering},
	},
	time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
	sync::{Mutex as AsyncMutex, OwnedMutexGuard, watch},
	task::JoinHandle,
};
use uuid::Uuid;
use zeroize::Zeroizing;

const KID: &str = "projects/app/locations/us-central1/keyRings/broker/cryptoKeys/tenant-secret/cryptoKeyVersions/1";
const AUDIENCE: &str = "env-tsec";
const VALUE: &str = "canary-tenant-secret-value-never-in-the-sandbox";
const HTTP_HOST: &str = "upstream.test";
const HTTP_ENDPOINT: &str = "https://upstream.test/v1/tools/invoke";
const MCP_ENDPOINT: &str = "https://upstream.test/v1/mcp";
const IMAGE_HOST: &str = "images.example.test";
const IMAGE_ENDPOINT: &str = "https://images.example.test/v1/";

fn now() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap()
		.as_secs()
}

fn run_subject() -> SecretSubject {
	SecretSubject::Run {
		run: "run-w7".into(),
		call: Uuid::new_v4(),
	}
}

// ---------------------------------------------------------------------------
// The fake Secret Store. It seals hosts with the value (ADR 0096) and is also
// the broker's SealedSecretSource, so the same bytes the server wrote are the
// bytes the broker opens.
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Version {
	payload: Vec<u8>,
	enabled: bool,
}

#[derive(Clone, Default)]
struct FakeStore {
	versions: Arc<Mutex<BTreeMap<String, BTreeMap<String, Version>>>>,
	reads: Arc<AtomicUsize>,
	store_unavailable: Arc<AtomicBool>,
}
impl FakeStore {
	fn put_payload(&self, resource: &str, version: &str, payload: Vec<u8>) {
		self.versions
			.lock()
			.unwrap()
			.entry(resource.into())
			.or_default()
			.insert(
				format!("{resource}/versions/{version}"),
				Version {
					payload,
					enabled: true,
				},
			);
	}
	fn disable_version(&self, resource: &str, version: &str) {
		if let Some(entry) = self
			.versions
			.lock()
			.unwrap()
			.get_mut(resource)
			.and_then(|versions| versions.get_mut(&format!("{resource}/versions/{version}")))
		{
			entry.enabled = false;
		}
	}
	fn reads(&self) -> usize {
		self.reads.load(Ordering::SeqCst)
	}
}
#[async_trait]
impl SecretStore for FakeStore {
	fn resource(&self, id: Uuid) -> String {
		format!("projects/1234/secrets/aidash-test-cred-{id}")
	}
	async fn create(&self, _: &str, id: Uuid) -> Result<(), Error> {
		self.versions
			.lock()
			.unwrap()
			.insert(self.resource(id), BTreeMap::new());
		Ok(())
	}
	async fn add_version(
		&self,
		_: &str,
		resource: &str,
		key: &SecretString,
	) -> Result<String, Error> {
		let mut versions = self.versions.lock().unwrap();
		let entry = versions.entry(resource.into()).or_default();
		let number = entry.len() + 1;
		let version = format!("{resource}/versions/{number}");
		entry.insert(
			version.clone(),
			Version {
				payload: key.expose_secret().as_bytes().to_vec(),
				enabled: true,
			},
		);
		Ok(version)
	}
	async fn versions(&self, _: &str, resource: &str) -> Result<Vec<String>, Error> {
		Ok(self
			.versions
			.lock()
			.unwrap()
			.get(resource)
			.map(|versions| versions.keys().cloned().collect())
			.unwrap_or_default())
	}
	async fn disable(&self, _: &str, version: &str) -> Result<(), Error> {
		for versions in self.versions.lock().unwrap().values_mut() {
			if let Some(entry) = versions.get_mut(version) {
				entry.enabled = false;
			}
		}
		Ok(())
	}
	async fn destroy(&self, _: &str, version: &str) -> Result<(), Error> {
		for versions in self.versions.lock().unwrap().values_mut() {
			if let Some(entry) = versions.get_mut(version) {
				entry.payload.clear();
				entry.enabled = false;
			}
		}
		Ok(())
	}
	async fn delete(&self, _: &str, resource: &str) -> Result<(), Error> {
		self.versions.lock().unwrap().remove(resource);
		Ok(())
	}
	async fn create_secret(&self, _: &str, id: Uuid) -> Result<(), Error> {
		self.versions
			.lock()
			.unwrap()
			.insert(self.secret_resource(id), BTreeMap::new());
		Ok(())
	}
	fn secret_resource(&self, id: Uuid) -> String {
		format!("projects/1234/secrets/aidash-test-tsec-{id}")
	}
	fn seals_secret_hosts(&self) -> bool {
		true
	}
	async fn add_secret_version(
		&self,
		_: &str,
		resource: &str,
		hosts: &[String],
		value: &SecretString,
	) -> Result<String, Error> {
		let sealed = sealed::seal(hosts, value.expose_secret())
			.map_err(|_| Error::Invalid("invalid sealed Tenant Secret payload".into()))?;
		let mut versions = self.versions.lock().unwrap();
		let entry = versions.entry(resource.into()).or_default();
		let number = entry.len() + 1;
		let version = format!("{resource}/versions/{number}");
		entry.insert(
			version.clone(),
			Version {
				payload: sealed.to_vec(),
				enabled: true,
			},
		);
		Ok(version)
	}
}
#[async_trait]
impl SealedSecretSource for FakeStore {
	async fn access(
		&self,
		secret: &str,
		version: &str,
	) -> Result<Zeroizing<Vec<u8>>, KeyMaterialError> {
		self.reads.fetch_add(1, Ordering::SeqCst);
		if self.store_unavailable.load(Ordering::SeqCst) {
			return Err(KeyMaterialError::StoreUnavailable);
		}
		match self
			.versions
			.lock()
			.unwrap()
			.get(secret)
			.and_then(|versions| versions.get(&format!("{secret}/versions/{version}")))
		{
			Some(entry) if entry.enabled => Ok(Zeroizing::new(entry.payload.clone())),
			_ => Err(KeyMaterialError::Unavailable),
		}
	}
}

// ---------------------------------------------------------------------------
// An in-memory Tenant Secret metadata repository, matching the application
// test fixture so no PostgreSQL is needed for the broker path.
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct Data {
	rows: BTreeMap<Uuid, TenantSecret>,
	events: Vec<(String, i64)>,
}
#[derive(Clone, Default)]
struct Repo(Arc<AsyncMutex<Data>>);
struct Lease {
	guard: OwnedMutexGuard<Data>,
	data: Data,
	tenant: String,
}
#[async_trait]
impl Repository for Repo {
	async fn begin(&self, tenant: &str) -> Result<Box<dyn Scope>, Error> {
		let guard = self.0.clone().lock_owned().await;
		let data = guard.clone();
		Ok(Box::new(Lease {
			guard,
			data,
			tenant: tenant.into(),
		}))
	}
	async fn reconciliation_candidates(
		&self,
		after: Option<Uuid>,
		limit: usize,
	) -> Result<Vec<TenantSecret>, Error> {
		Ok(self
			.0
			.lock()
			.await
			.rows
			.values()
			.filter(|row| after.is_none_or(|id| row.id > id))
			.take(limit)
			.cloned()
			.collect())
	}
	async fn all(&self, tenant: &str) -> Result<Vec<TenantSecret>, Error> {
		Ok(self
			.0
			.lock()
			.await
			.rows
			.values()
			.filter(|row| row.tenant == tenant)
			.cloned()
			.collect())
	}
}
#[async_trait]
impl Scope for Lease {
	async fn named(&mut self, name: &str) -> Result<Option<TenantSecret>, Error> {
		Ok(self
			.data
			.rows
			.values()
			.find(|row| {
				row.tenant == self.tenant
					&& row.name == name
					&& row.state != aidash_domain::tenant_secrets::State::Deleted
			})
			.cloned())
	}
	async fn by_id(&mut self, id: Uuid) -> Result<Option<TenantSecret>, Error> {
		Ok(self.data.rows.get(&id).cloned())
	}
	async fn list(&mut self) -> Result<Vec<TenantSecret>, Error> {
		Ok(self
			.data
			.rows
			.values()
			.filter(|row| {
				row.tenant == self.tenant
					&& row.state != aidash_domain::tenant_secrets::State::Deleted
			})
			.cloned()
			.collect())
	}
	async fn count(&mut self) -> Result<usize, Error> {
		Ok(self.list().await?.len())
	}
	async fn insert(&mut self, value: &TenantSecret) -> Result<(), Error> {
		self.data.rows.insert(value.id, value.clone());
		Ok(())
	}
	async fn save(&mut self, value: &TenantSecret, event: &str, _: &str) -> Result<(), Error> {
		self.data.rows.insert(value.id, value.clone());
		self.data.events.push((event.into(), value.revision));
		Ok(())
	}
	async fn remove(&mut self, id: Uuid) -> Result<(), Error> {
		self.data.rows.remove(&id);
		Ok(())
	}
	async fn commit(mut self: Box<Self>) -> Result<(), Error> {
		*self.guard = self.data.clone();
		Ok(())
	}
}

// ---------------------------------------------------------------------------
// Brokers, upstreams and the server-side service.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Logs(Mutex<Vec<String>>);
impl SecretCallSink for Logs {
	fn record(&self, audit: &SecretCallAudit) {
		self.0
			.lock()
			.unwrap()
			.push(serde_json::to_string(audit).unwrap());
	}
}
impl Logs {
	fn entries(&self) -> Vec<Value> {
		self.0
			.lock()
			.unwrap()
			.iter()
			.map(|line| serde_json::from_str(line).unwrap())
			.collect()
	}
}

#[derive(Default)]
struct Seen {
	method: String,
	path: String,
	headers: HeaderMap,
	body: Vec<u8>,
}

#[derive(Default)]
struct Upstream {
	calls: AtomicUsize,
	seen: Mutex<Vec<Seen>>,
	sse_calls: AtomicUsize,
	session_round_trip: AtomicBool,
}

async fn upstream_handler(State(state): State<Arc<Upstream>>, request: Request) -> Response {
	state.calls.fetch_add(1, Ordering::SeqCst);
	let method = request.method().as_str().to_owned();
	let path = request.uri().path().to_owned();
	let headers = request.headers().clone();
	let body = to_bytes(request.into_body(), 1024 * 1024)
		.await
		.unwrap()
		.to_vec();
	state.seen.lock().unwrap().push(Seen {
		method: method.clone(),
		path: path.clone(),
		headers: headers.clone(),
		body: body.clone(),
	});
	if path == "/v1/mcp" && method == "GET" {
		return StatusCode::METHOD_NOT_ALLOWED.into_response();
	}
	if path == "/v1/mcp" && method == "DELETE" {
		return StatusCode::OK.into_response();
	}
	if path == "/v1/mcp" {
		let message: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
		match message["method"].as_str().unwrap_or_default() {
			"initialize" => {
				let mut response = Json(json!({
					"jsonrpc": "2.0",
					"id": message["id"],
					"result": {
						"protocolVersion": message["params"]["protocolVersion"],
						"capabilities": {"tools": {}},
						"serverInfo": {"name": "w7-upstream", "version": "1.0.0"}
					}
				}))
				.into_response();
				response
					.headers_mut()
					.insert("mcp-session-id", "w7-session".parse().unwrap());
				return response;
			}
			"tools/call" => {
				if headers
					.get("mcp-session-id")
					.and_then(|value| value.to_str().ok())
					== Some("w7-session")
				{
					state.session_round_trip.store(true, Ordering::SeqCst);
				}
				state.sse_calls.fetch_add(1, Ordering::SeqCst);
				let event = json!({
					"jsonrpc": "2.0",
					"id": message["id"],
					"result": {"content": [{"type": "text", "text": "through the real broker"}]}
				});
				return Response::builder()
					.status(StatusCode::OK)
					.header(header::CONTENT_TYPE, "text/event-stream")
					.body(Body::from(format!("event: message\ndata: {event}\n\n")))
					.unwrap();
			}
			_ => return StatusCode::ACCEPTED.into_response(),
		}
	}
	Json(json!({
		"method": method,
		"path": path,
		"body": String::from_utf8_lossy(&body),
	}))
	.into_response()
}

async fn serve(router: Router) -> (String, JoinHandle<()>) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let base = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(listener, router).await.unwrap();
	});
	(base, server)
}

struct Harness {
	store: FakeStore,
	service: Arc<Service>,
	adapter: Arc<SecretCallsAdapter>,
	signer: Arc<InMemorySigner>,
	_broker: Arc<TenantSecretBroker>,
	broker_base: String,
	broker_server: JoinHandle<()>,
	logs: Arc<Logs>,
	upstream: Arc<Upstream>,
	upstream_base: String,
	upstream_server: JoinHandle<()>,
}
impl Drop for Harness {
	fn drop(&mut self) {
		self.broker_server.abort();
		self.upstream_server.abort();
	}
}
impl Harness {
	async fn new() -> Self {
		let upstream = Arc::new(Upstream::default());
		let upstream_router = Router::new()
			.fallback(any(upstream_handler))
			.with_state(upstream.clone());
		let (upstream_base, upstream_server) = serve(upstream_router).await;

		let store = FakeStore::default();
		let logs = Arc::new(Logs::default());
		let signer = Arc::new(InMemorySigner::new(KID.into(), [7; 32]));
		let mut keys = PublicKeys::default();
		keys.insert(KID.into(), signer.public_key());
		let mut broker = TenantSecretBroker::new(
			TenantSecretConfig {
				byok_project_number: "1234".into(),
				secret_prefix: "aidash-test-tsec-".into(),
				audience: AUDIENCE.into(),
				issuers: BTreeMap::from([(KID.into(), "aidash-server".into())]),
				requests_per_second: 100.0,
				burst: 100,
				timeout: Duration::from_secs(5),
			},
			keys,
			Arc::new(store.clone()),
			logs.clone(),
		)
		.unwrap();
		// The only test seam: the token's destination is still `https://upstream.test`,
		// checked against the public-address rule and the sealed hosts.
		broker.test_origin(upstream_base.clone());
		let broker = Arc::new(broker);
		let broker_router = broker.clone().router();
		let (broker_base, broker_server) = serve(broker_router).await;

		let adapter = Arc::new(
			SecretCallsAdapter::with_signer(
				&broker_base,
				SecretIssuerConfiguration {
					issuer: "aidash-server".into(),
					audience: AUDIENCE.into(),
					kid: KID.into(),
				},
				signer.clone(),
			)
			.unwrap(),
		);
		let repo = Repo::default();
		let service = Arc::new(Service {
			repository: Arc::new(repo),
			store: Arc::new(store.clone()),
			// The application host is write-only (ADR 0002): no reader exists.
			reader: None,
			calls: Some(adapter.clone()),
			fingerprint_key: "w7-integration-fingerprint-key-at-least-32-bytes".into(),
			max_per_tenant: 64,
		});
		Self {
			store,
			service,
			adapter,
			signer,
			_broker: broker,
			broker_base,
			broker_server,
			logs,
			upstream,
			upstream_base,
			upstream_server,
		}
	}

	fn tools(&self, permitted: &[&str]) -> HttpTools {
		HttpTools::new(
			KeylessAccess::permitting(PermittedOrigins::parse(permitted).unwrap()),
			ToolClients::new().unwrap(),
		)
	}

	async fn put_secret(
		&self,
		tenant: &str,
		name: &str,
		hosts: &[&str],
	) -> aidash_application::tenant_secrets::Metadata {
		self.service
			.put(
				tenant,
				name,
				Put {
					expected_revision: 0,
					value: Some(SecretString::from(VALUE)),
					hosts: Some(hosts.iter().map(|host| (*host).to_owned()).collect()),
				},
				"actor",
			)
			.await
			.unwrap()
	}

	fn claims(&self, tenant: &str, secret: Uuid, origin: &str, prefix: &str) -> SecretClaims {
		SecretClaims {
			iss: "aidash-server".into(),
			aud: AUDIENCE.into(),
			iat: now(),
			exp: now() + 60,
			jti: Uuid::new_v4(),
			kid: KID.into(),
			tenant: tenant.into(),
			secret,
			version: "1".into(),
			destination: Destination {
				origin: origin.into(),
				path_prefix: prefix.into(),
			},
			methods: vec!["GET".into(), "POST".into(), "DELETE".into()],
			attach: Attach::Bearer,
			sub: run_subject(),
			max_response_bytes: None,
		}
	}

	async fn mint(&self, claims: &SecretClaims) -> SecretString {
		mint_secret(claims, self.signer.as_ref()).await.unwrap()
	}

	async fn forward(
		&self,
		token: &str,
		method: &str,
		path: &str,
		body: &str,
	) -> reqwest::Response {
		reqwest::Client::builder()
			.no_proxy()
			.build()
			.unwrap()
			.request(
				method.parse().unwrap(),
				format!("{}{path}", self.broker_base),
			)
			.header("x-aidash-secret-call", token)
			.header(header::CONTENT_TYPE, "application/json")
			.body(body.to_owned())
			.send()
			.await
			.unwrap()
	}

	async fn call_secret(&self, endpoint: &str, methods: &[&str]) -> CallSecret {
		let secret = self
			.service
			.call_secret(
				"tenant-a",
				"search",
				endpoint,
				SecretCall {
					destination: endpoint_destination(endpoint).unwrap(),
					methods: methods.iter().map(|method| (*method).to_owned()).collect(),
					attach: Attach::Bearer,
					subject: run_subject(),
					max_response_bytes: Some(256_000),
				},
			)
			.await
			.unwrap();
		assert!(matches!(secret, CallSecret::Brokered { .. }));
		secret
	}
}

fn http_tool(endpoint: &str) -> ToolConfig {
	ToolConfig::Http {
		endpoint: endpoint.into(),
		replay: "read_only".into(),
		secret: Some("search".into()),
	}
}

fn mcp_tool(endpoint: &str) -> ToolConfig {
	ToolConfig::Mcp {
		endpoint: endpoint.into(),
		tool_name: "lookup".into(),
		replay: "read_only".into(),
		idempotency_argument: None,
		secret: Some("search".into()),
	}
}

// ---------------------------------------------------------------------------
// A.1 HTTP tool call -> broker -> upstream.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_http_tool_call_carries_the_value_only_between_the_broker_and_upstream() {
	let h = Harness::new().await;
	let metadata = h.put_secret("tenant-a", "search", &[HTTP_HOST]).await;
	let call = h.call_secret(HTTP_ENDPOINT, &["POST"]).await;
	let result = h
		.tools(&["https://upstream.test"])
		.invoke(
			&http_tool(HTTP_ENDPOINT),
			json!({"query":"w7"}),
			"call-key",
			Some(&call),
		)
		.await
		.unwrap();
	assert_eq!(result["path"], "/v1/tools/invoke");
	assert!(!result.to_string().contains(VALUE));

	let seen = h.upstream.seen.lock().unwrap();
	assert_eq!(seen.len(), 1);
	assert_eq!(seen[0].method, "POST");
	assert_eq!(seen[0].path, "/v1/tools/invoke");
	assert_eq!(
		seen[0].headers[header::AUTHORIZATION].to_str().unwrap(),
		format!("Bearer {VALUE}")
	);
	assert!(
		!seen[0].headers.contains_key("x-aidash-secret-call"),
		"the caller's token must never reach the tool endpoint"
	);
	assert!(
		String::from_utf8_lossy(&seen[0].body).contains("w7"),
		"the tool input reached the endpoint"
	);
	assert!(seen[0].headers.get(header::AUTHORIZATION).is_some());
	drop(seen);

	// The broker read the sealed version once; the application host holds no
	// reader at all.
	assert_eq!(h.store.reads(), 1);
	assert!(h.service.calls.is_some());
	assert!(h.service.reader.is_none());

	let logs = h.logs.entries();
	assert_eq!(logs.len(), 1);
	assert_eq!(logs[0]["tenant"], "tenant-a");
	assert_eq!(logs[0]["secret"], metadata.id.to_string());
	assert_eq!(logs[0]["version"], "1");
	assert_eq!(logs[0]["host"], HTTP_HOST);
	assert_eq!(logs[0]["status"], 200);
	assert!(!logs[0].to_string().contains(VALUE));
}

// ---------------------------------------------------------------------------
// A.2 MCP Streamable HTTP session -> broker -> upstream.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_mcp_streamable_http_session_passes_through_the_broker_with_its_session() {
	let h = Harness::new().await;
	h.put_secret("tenant-a", "search", &[HTTP_HOST]).await;
	let call = h
		.call_secret(MCP_ENDPOINT, &["POST", "GET", "DELETE"])
		.await;
	let result = h
		.tools(&["https://upstream.test"])
		.invoke(
			&mcp_tool(MCP_ENDPOINT),
			json!({"query":"mcp"}),
			"call-key",
			Some(&call),
		)
		.await
		.unwrap();
	assert_eq!(result["content"][0]["text"], "through the real broker");
	assert!(h.upstream.session_round_trip.load(Ordering::SeqCst));
	assert_eq!(h.upstream.sse_calls.load(Ordering::SeqCst), 1);

	let seen = h.upstream.seen.lock().unwrap();
	assert!(seen.iter().any(|seen| seen.path == "/v1/mcp"));
	for seen in seen.iter().filter(|seen| seen.path == "/v1/mcp") {
		assert_eq!(
			seen.headers[header::AUTHORIZATION].to_str().unwrap(),
			format!("Bearer {VALUE}")
		);
		assert!(
			!seen.headers.contains_key("x-aidash-secret-call"),
			"the MCP endpoint must never receive the broker token"
		);
	}
	assert!(
		seen.iter()
			.any(|seen| seen.headers.get("mcp-session-id").is_some()),
		"the session id reached the tool endpoint"
	);
}

// ---------------------------------------------------------------------------
// A.3 Egress gateway -> broker -> upstream, and a no-secret service direct.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct GatewayLog {
	sent: AtomicUsize,
	received: AtomicUsize,
	finished: Mutex<Vec<(u16, Option<String>)>>,
}
#[async_trait]
impl Meter for GatewayLog {
	async fn charge(&self, direction: Direction, bytes: usize) -> Result<(), Error> {
		match direction {
			Direction::Sent => &self.sent,
			Direction::Received => &self.received,
		}
		.fetch_add(bytes, Ordering::Relaxed);
		Ok(())
	}
	async fn finish(&self, status: u16, _: u64, error: Option<&str>) -> Result<(), Error> {
		self.finished
			.lock()
			.unwrap()
			.push((status, error.map(str::to_owned)));
		Ok(())
	}
}

struct GatewayAuthority {
	direct_base: String,
	route: Option<aidash_application::tenant_secrets::SecretRoute>,
	log: Arc<GatewayLog>,
}
#[async_trait]
impl Authority for GatewayAuthority {
	async fn admit(
		&self,
		token: &str,
		service: &str,
		_method: &str,
		_path: &str,
		_digest: &str,
	) -> Result<Admission, Error> {
		if token != "operation-token" {
			return Err(Error::Forbidden);
		}
		let (definition, route) = match service {
			"images" => (
				ExternalService {
					base_url: IMAGE_ENDPOINT.into(),
					methods: vec!["GET".into(), "POST".into()],
					secret: Some("image-key".into()),
					authentication: Authentication::Bearer,
					limits: None,
				},
				self.route.clone(),
			),
			"direct" => (
				ExternalService {
					base_url: self.direct_base.clone(),
					methods: vec!["GET".into()],
					secret: None,
					authentication: Authentication::Bearer,
					limits: None,
				},
				None,
			),
			_ => return Err(Error::Forbidden),
		};
		Ok(Admission {
			gateway_origin: "http://gateway.test".into(),
			destination: Address::Permitted,
			request: Uuid::new_v4(),
			operation: Uuid::new_v4(),
			session: None,
			service: service.into(),
			definition,
			expires_at: chrono::Utc::now() + chrono::Duration::seconds(30),
			credential: None,
			route,
			meter: self.log.clone(),
		})
	}
}

#[tokio::test]
async fn the_egress_gateway_forwards_a_secret_service_and_not_a_plain_one() {
	let h = Harness::new().await;
	h.put_secret("tenant-a", "image-key", &[IMAGE_HOST]).await;
	let route = match h
		.service
		.resolve(
			"tenant-a",
			"image-key",
			IMAGE_ENDPOINT,
			Some(SecretCall {
				destination: endpoint_destination(IMAGE_ENDPOINT).unwrap(),
				methods: vec!["GET".into(), "POST".into()],
				attach: Attach::Bearer,
				subject: SecretSubject::Operation {
					operation: Uuid::new_v4(),
					tenant: "tenant-a".into(),
				},
				max_response_bytes: None,
			}),
		)
		.await
		.unwrap()
	{
		Resolved::Brokered(route) => route,
		Resolved::Value(_) => panic!("the Cloud path never reads a value"),
	};
	let log = Arc::new(GatewayLog::default());
	let authority = Arc::new(GatewayAuthority {
		direct_base: format!("{}/", h.upstream_base),
		route: Some(route),
		log: log.clone(),
	});
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let gateway_base = format!("http://{}", listener.local_addr().unwrap());
	let (stop, receiver) = watch::channel(false);
	let gateway = tokio::spawn(egress::serve(listener, authority, receiver));
	let client = reqwest::Client::builder().no_proxy().build().unwrap();

	let response = client
		.get(format!(
			"{gateway_base}/v1/operation-token/images/upload?q=1"
		))
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), StatusCode::OK);
	let body = response.text().await.unwrap();
	assert!(!body.contains(VALUE), "the sandbox saw the value: {body}");
	{
		let seen = h.upstream.seen.lock().unwrap();
		assert_eq!(seen.last().unwrap().path, "/v1/upload");
		assert_eq!(
			seen.last().unwrap().headers[header::AUTHORIZATION]
				.to_str()
				.unwrap(),
			format!("Bearer {VALUE}")
		);
		assert!(
			!seen
				.last()
				.unwrap()
				.headers
				.contains_key("x-aidash-secret-call")
		);
	}

	let broker_calls = h.logs.entries().len();
	let plain = client
		.get(format!("{gateway_base}/v1/operation-token/direct/ping"))
		.send()
		.await
		.unwrap();
	assert_eq!(plain.status(), StatusCode::OK);
	assert!(!plain.text().await.unwrap().contains("canary"));
	{
		let seen = h.upstream.seen.lock().unwrap();
		assert_eq!(seen.last().unwrap().path, "/ping");
		assert!(
			seen.last()
				.unwrap()
				.headers
				.get(header::AUTHORIZATION)
				.is_none()
		);
	}
	assert_eq!(
		h.logs.entries().len(),
		broker_calls,
		"a no-secret External service must not reach the broker"
	);
	assert_eq!(log.finished.lock().unwrap().len(), 2);

	stop.send(true).unwrap();
	gateway.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// A.4 A token is bound to its destination and its Tenant Secret, and token
// kinds/audiences/issuers are separate.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tokens_are_refused_for_another_destination_secret_kind_audience_or_issuer() {
	let h = Harness::new().await;
	let a = h.put_secret("tenant-a", "search", &[HTTP_HOST]).await;
	let b = h.put_secret("tenant-a", "other", &["other.test"]).await;
	assert_ne!(a.id, b.id);

	let minted = |grant: SecretCallGrant| {
		let adapter = h.adapter.clone();
		async move { adapter.route(grant).await.unwrap() }
	};
	// A token minted for secret A and destination A.
	let route_a = minted(SecretCallGrant {
		tenant: "tenant-a".into(),
		secret: a.id,
		version: "1".into(),
		destination: endpoint_destination(HTTP_ENDPOINT).unwrap(),
		methods: vec!["GET".into(), "POST".into()],
		attach: Attach::Bearer,
		subject: run_subject(),
		max_response_bytes: None,
	})
	.await;
	// The same token cannot reach a path outside its signed prefix.
	let refused = h
		.forward(
			route_a.token.expose_secret(),
			"POST",
			"/v1/forward/%2e%2e/other",
			"{}",
		)
		.await;
	assert_eq!(refused.status(), StatusCode::FORBIDDEN);
	assert_eq!(
		refused.headers()["x-aidash-broker-error"],
		"secret_destination_refused"
	);
	assert_eq!(h.upstream.calls.load(Ordering::SeqCst), 0);

	// A token the real adapter mints with secret A for another destination is
	// refused by the sealed hosts, even though the signature and claims are valid.
	let cross_destination = minted(SecretCallGrant {
		tenant: "tenant-a".into(),
		secret: a.id,
		version: "1".into(),
		destination: endpoint_destination("https://other.test/v1/tools/invoke").unwrap(),
		methods: vec!["POST".into()],
		attach: Attach::Bearer,
		subject: run_subject(),
		max_response_bytes: None,
	})
	.await;
	let refused = h
		.forward(
			cross_destination.token.expose_secret(),
			"POST",
			"/v1/forward/tools/invoke",
			"{}",
		)
		.await;
	assert_eq!(refused.status(), StatusCode::FORBIDDEN);
	assert_eq!(h.upstream.calls.load(Ordering::SeqCst), 0);

	// A token minted for secret B while the call names secret A is refused the
	// same way.
	let cross_secret = minted(SecretCallGrant {
		tenant: "tenant-a".into(),
		secret: b.id,
		version: "1".into(),
		destination: endpoint_destination(HTTP_ENDPOINT).unwrap(),
		methods: vec!["POST".into()],
		attach: Attach::Bearer,
		subject: run_subject(),
		max_response_bytes: None,
	})
	.await;
	let refused = h
		.forward(
			cross_secret.token.expose_secret(),
			"POST",
			"/v1/forward/tools/invoke",
			"{}",
		)
		.await;
	assert_eq!(refused.status(), StatusCode::FORBIDDEN);
	assert_eq!(h.upstream.calls.load(Ordering::SeqCst), 0);

	// Wrong audience, wrong issuer for this kid, and the provider token kind are
	// all broker rejections.
	let mut wrong_audience = h.claims("tenant-a", a.id, "https://upstream.test", "/v1/tools/");
	wrong_audience.aud = "other".into();
	let refused = h
		.forward(
			h.mint(&wrong_audience).await.expose_secret(),
			"POST",
			"/v1/forward/tools/invoke",
			"{}",
		)
		.await;
	assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);

	let mut wrong_issuer = h.claims("tenant-a", a.id, "https://upstream.test", "/v1/tools/");
	wrong_issuer.iss = "aidash-worker".into();
	let refused = h
		.forward(
			h.mint(&wrong_issuer).await.expose_secret(),
			"POST",
			"/v1/forward/tools/invoke",
			"{}",
		)
		.await;
	assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);

	let provider = Claims {
		iss: "aidash-server".into(),
		aud: AUDIENCE.into(),
		iat: now(),
		exp: now() + 60,
		jti: Uuid::new_v4(),
		kid: KID.into(),
		tenant: "tenant-a".into(),
		provider: "openrouter".into(),
		credential: Uuid::new_v4(),
		version: "1".into(),
		sub: TokenSubject::Run {
			run: "run-w7".into(),
			call: Uuid::new_v4(),
		},
		ops: vec![Operation::Chat],
		model: "author/model".into(),
		max_output_tokens: 10,
		cache_breakpoints: false,
		hosts: vec![],
	};
	let provider_token = mint(&provider, h.signer.as_ref()).await.unwrap();
	let refused = h
		.forward(
			provider_token.expose_secret(),
			"POST",
			"/v1/forward/tools/invoke",
			"{}",
		)
		.await;
	assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);

	assert_eq!(h.upstream.calls.load(Ordering::SeqCst), 0);
	assert!(
		h.store.reads() >= 1,
		"secret A was read for the accepted token only"
	);
}

// ---------------------------------------------------------------------------
// A.5 Sealed hosts are the authority; changing metadata alone cannot move a
// value, and a sealing Store refuses a hosts-only change.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sealed_hosts_and_the_sealing_store_refuse_a_hosts_change_without_the_value() {
	let h = Harness::new().await;
	let metadata = h.put_secret("tenant-a", "search", &[HTTP_HOST]).await;
	// Simulate the metadata row changing its hosts without re-sealing: the
	// stored payload still names the old host.
	let resource = format!("projects/1234/secrets/aidash-test-tsec-{}", metadata.id);
	h.store.put_payload(
		&resource,
		"1",
		sealed::seal(&["elsewhere.test".into()], VALUE)
			.unwrap()
			.to_vec(),
	);
	let token = h
		.mint(&h.claims(
			"tenant-a",
			metadata.id,
			"https://upstream.test",
			"/v1/tools/",
		))
		.await;
	let refused = h
		.forward(
			token.expose_secret(),
			"POST",
			"/v1/forward/tools/invoke",
			"{}",
		)
		.await;
	assert_eq!(refused.status(), StatusCode::FORBIDDEN);
	assert_eq!(
		refused.headers()["x-aidash-broker-error"],
		"secret_destination_refused"
	);
	assert_eq!(h.upstream.calls.load(Ordering::SeqCst), 0);

	// ADR 0096: a sealing Store cannot attach new hosts to a value the
	// application host cannot read.
	let error = h
		.service
		.put(
			"tenant-a",
			"search",
			Put {
				expected_revision: metadata.revision,
				value: None,
				hosts: Some(vec!["new.example.test".into()]),
			},
			"actor",
		)
		.await
		.unwrap_err();
	assert!(
		error
			.to_string()
			.contains("requires its value on this Deployment"),
		"{error}"
	);
}

// ---------------------------------------------------------------------------
// A.6 Version unavailable is repairable; a broker outage is a Deployment Administrator fault
// with no direct fallback.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unavailable_version_maps_to_the_repairable_secret_unavailable_error() {
	let h = Harness::new().await;
	let metadata = h.put_secret("tenant-a", "search", &[HTTP_HOST]).await;
	// The value's version is gone while the metadata still names it (a revoke
	// that has not reached the metadata row, or a destroyed version).
	let resource = format!("projects/1234/secrets/aidash-test-tsec-{}", metadata.id);
	h.store.disable_version(&resource, "1");
	let call = h.call_secret(HTTP_ENDPOINT, &["POST"]).await;
	let error = h
		.tools(&["https://upstream.test"])
		.invoke(
			&http_tool(HTTP_ENDPOINT),
			json!({}),
			"call-key",
			Some(&call),
		)
		.await
		.unwrap_err();
	assert!(
		matches!(&error, Error::Invalid(message) if message == "secret_unavailable"),
		"{error:?}"
	);
	assert_eq!(h.upstream.calls.load(Ordering::SeqCst), 0);

	// A deleted Tenant Secret is the same repairable refusal before any broker
	// call is minted.
	h.service
		.delete("tenant-a", "search", metadata.revision, "actor")
		.await
		.unwrap();
	let error = match h
		.service
		.resolve(
			"tenant-a",
			"search",
			HTTP_ENDPOINT,
			Some(SecretCall {
				destination: endpoint_destination(HTTP_ENDPOINT).unwrap(),
				methods: vec!["POST".into()],
				attach: Attach::Bearer,
				subject: run_subject(),
				max_response_bytes: None,
			}),
		)
		.await
	{
		Ok(_) => panic!("a deleted Tenant Secret must not resolve"),
		Err(error) => error,
	};
	assert!(
		matches!(&error, Error::Invalid(message) if message == "secret_unavailable"),
		"{error:?}"
	);
}

#[tokio::test]
async fn a_broker_outage_is_an_deployment_administrator_fault_and_never_falls_back_to_a_direct_call()
 {
	let mut h = Harness::new().await;
	h.put_secret("tenant-a", "search", &[HTTP_HOST]).await;
	let call = h.call_secret(HTTP_ENDPOINT, &["POST"]).await;
	// The route was minted; take the broker down before the transport sends it.
	h.broker_server.abort();
	let _ = (&mut h.broker_server).await;

	let error = h
		.tools(&["https://upstream.test"])
		.invoke(
			&http_tool(HTTP_ENDPOINT),
			json!({}),
			"call-key",
			Some(&call),
		)
		.await
		.unwrap_err();
	assert!(
		matches!(&error, Error::BrokerUnavailable { correlation: None }),
		"{error:?}"
	);
	assert_eq!(
		h.upstream.calls.load(Ordering::SeqCst),
		0,
		"a broker outage must not retry the tool endpoint directly"
	);
}

#[tokio::test]
async fn a_secret_store_outage_is_broker_unavailable_not_a_direct_read() {
	let h = Harness::new().await;
	h.put_secret("tenant-a", "search", &[HTTP_HOST]).await;
	h.store.store_unavailable.store(true, Ordering::SeqCst);
	let call = h.call_secret(HTTP_ENDPOINT, &["POST"]).await;
	let error = h
		.tools(&["https://upstream.test"])
		.invoke(
			&http_tool(HTTP_ENDPOINT),
			json!({}),
			"call-key",
			Some(&call),
		)
		.await
		.unwrap_err();
	assert!(
		matches!(
			&error,
			Error::BrokerUnavailable {
				correlation: Some(_)
			}
		),
		"{error:?}"
	);
	assert_eq!(h.upstream.calls.load(Ordering::SeqCst), 0);
}
