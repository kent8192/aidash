use super::*;
use aidash_capability::{
	InMemorySigner, PublicKeys,
	connection::mint_connection,
	tenant_secret::{Destination, SealedSecretSource, SecretSubject},
};
use axum::{
	body::{Body, to_bytes},
	http::Request,
};
use std::collections::BTreeMap;
use tower::ServiceExt;
use wiremock::{
	Mock, MockServer, ResponseTemplate,
	matchers::{body_string_contains, header, method, path},
};
use zeroize::Zeroizing;
#[derive(Default)]
struct Store(Mutex<BTreeMap<(String, String), Vec<u8>>>);
#[async_trait::async_trait]
impl ConnectionStore for Store {
	async fn access(&self, r: &str, v: &str) -> Result<Zeroizing<Vec<u8>>, KeyMaterialError> {
		self.0
			.lock()
			.unwrap()
			.get(&(r.into(), v.into()))
			.cloned()
			.map(Zeroizing::new)
			.ok_or(KeyMaterialError::Unavailable)
	}
	async fn latest(&self, r: &str) -> Result<(String, Zeroizing<Vec<u8>>), KeyMaterialError> {
		self.0
			.lock()
			.unwrap()
			.iter()
			.filter(|((k, _), _)| k == r)
			.max_by_key(|((_, v), _)| v.parse::<u64>().unwrap())
			.map(|((_, v), p)| (v.clone(), Zeroizing::new(p.clone())))
			.ok_or(KeyMaterialError::Unavailable)
	}
	async fn add_version(&self, r: &str, p: &[u8]) -> Result<String, KeyMaterialError> {
		let mut data = self.0.lock().unwrap();
		let n = data.keys().filter(|(k, _)| k == r).count() + 1;
		let v = n.to_string();
		data.insert((r.into(), v.clone()), p.to_vec());
		Ok(v)
	}
}
#[async_trait::async_trait]
impl SealedSecretSource for Store {
	async fn access(&self, _: &str, _: &str) -> Result<Zeroizing<Vec<u8>>, KeyMaterialError> {
		panic!("OAuth must not read pasted-secret source")
	}
}
#[derive(Default)]
struct Logs(Mutex<Vec<String>>);
impl crate::tenant_secret::SecretCallSink for Logs {
	fn record(&self, a: &SecretCallAudit) {
		self.0
			.lock()
			.unwrap()
			.push(serde_json::to_string(a).unwrap());
	}
}
struct Fixture {
	broker: Arc<TenantSecretBroker>,
	store: Arc<Store>,
	signer: InMemorySigner,
	logs: Arc<Logs>,
	connection: Uuid,
	client: Uuid,
}
impl Fixture {
	fn new(origin: &str) -> Self {
		let signer = InMemorySigner::new("key".into(), [8; 32]);
		let mut keys = PublicKeys::default();
		keys.insert("key".into(), signer.public_key());
		let store = Arc::new(Store::default());
		let logs = Arc::new(Logs::default());
		let config = crate::tenant_secret::TenantSecretConfig {
			byok_project_number: "123".into(),
			secret_prefix: "aidash-dev-tsec-".into(),
			audience: "dev-tsec".into(),
			issuers: BTreeMap::from([("key".into(), "server".into())]),
			requests_per_second: 100.,
			burst: 100,
			timeout: Duration::from_secs(10),
		};
		let mut broker = TenantSecretBroker::new(config, keys, store.clone(), logs.clone())
			.unwrap()
			.with_connections(ConnectionBroker::for_test(store.clone()));
		broker.test_origin(origin.into());
		Self {
			broker: Arc::new(broker),
			store,
			signer,
			logs,
			connection: Uuid::now_v7(),
			client: Uuid::now_v7(),
		}
	}
	fn resource(&self, id: Uuid) -> String {
		format!("projects/123/secrets/aidash-dev-oauth-{id}")
	}
	async fn call(&self, path: &str, operation: ConnectionOperation, body: &str) -> Response {
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap()
			.as_secs();
		let c = ConnectionClaims {
			iss: "server".into(),
			aud: "dev-tsec".into(),
			iat: now,
			exp: now + 60,
			jti: Uuid::new_v4(),
			kid: "key".into(),
			tenant: "tenant".into(),
			connection: self.connection,
			operation,
		};
		let token = mint_connection(&c, &self.signer).await.unwrap();
		self.broker
			.clone()
			.router()
			.oneshot(
				Request::builder()
					.method("POST")
					.uri(path)
					.header("x-aidash-secret-call", token.expose_secret())
					.body(Body::from(body.to_owned()))
					.unwrap(),
			)
			.await
			.unwrap()
	}
	fn sub(&self) -> SecretSubject {
		SecretSubject::Operation {
			operation: Uuid::new_v4(),
			tenant: "tenant".into(),
		}
	}
	fn exchange(&self, issuer: &str) -> ConnectionOperation {
		ConnectionOperation::Exchange {
			issuer: issuer.into(),
			resource: "https://mcp.example/mcp".into(),
			client_id: "registered".into(),
			client: Some(ClientSecretRef {
				kind: ClientKind::Registered,
				id: self.client,
			}),
			redirect_uri: "https://aidash.example/callback".into(),
			sub: self.sub(),
		}
	}
	fn forward(&self, version: &str, origin: &str, prefix: &str) -> ConnectionOperation {
		ConnectionOperation::Forward {
			version: version.into(),
			destination: Destination {
				origin: origin.into(),
				path_prefix: prefix.into(),
			},
			methods: vec!["POST".into()],
			sub: SecretSubject::Run {
				run: "run".into(),
				call: Uuid::new_v4(),
			},
			max_response_bytes: None,
		}
	}
}
async fn json(response: Response) -> serde_json::Value {
	serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap()
}
async fn metadata(server: &MockServer) {
	Mock::given(method("GET"))
		.and(path("/.well-known/oauth-authorization-server"))
		.respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"issuer":server.uri(),"authorization_endpoint":format!("{}/authorize",server.uri()),"token_endpoint":format!("{}/token",server.uri()),"revocation_endpoint":format!("{}/revoke",server.uri()),"code_challenge_methods_supported":["S256"]})))
		.expect(1)
		.mount(server)
		.await;
}
#[tokio::test]
async fn exchange_refresh_pinned_forward_and_revoke_keep_all_tokens_inside_broker() {
	let server = MockServer::start().await;
	metadata(&server).await;
	let f = Fixture::new(&server.uri());
	let payload = sealed::seal(Payload::RegisteredClient {
		issuer: server.uri(),
		client_id: "registered".into(),
		client_secret: "client-canary".into(),
	})
	.unwrap();
	f.store
		.0
		.lock()
		.unwrap()
		.insert((f.resource(f.client), "1".into()), payload.to_vec());
	Mock::given(method("POST"))
		.and(path("/token"))
		.and(body_string_contains("grant_type=authorization_code"))
		.and(body_string_contains("client_secret=client-canary"))
		.and(body_string_contains("client_id=registered"))
		.and(body_string_contains("code=code-canary"))
		.and(body_string_contains("code_verifier=verifier-canary"))
		.respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"access_token":"access-1-canary","token_type":"Bearer","refresh_token":"refresh-1-canary","expires_in":1,"scope":"read"})))
		.expect(1)
		.mount(&server)
		.await;
	let response = f
		.call(
			"/v1/connections/exchange",
			f.exchange(&server.uri()),
			r#"{"code":"code-canary","verifier":"verifier-canary"}"#,
		)
		.await;
	assert_eq!(response.status(), 200);
	let receipt = json(response).await;
	assert_eq!(
		receipt,
		serde_json::json!({"version":"1","expires_in":1,"scope":"read","refresh":true})
	);
	// Replacing the registered secret removes its old pin, as the management
	// service does. The existing connection must use the validated latest one.
	let rotated = sealed::seal(Payload::RegisteredClient {
		issuer: server.uri(),
		client_id: "registered".into(),
		client_secret: "rotated-client-canary".into(),
	})
	.unwrap();
	{
		let mut data = f.store.0.lock().unwrap();
		data.insert((f.resource(f.client), "2".into()), rotated.to_vec());
		data.remove(&(f.resource(f.client), "1".into()));
	}
	Mock::given(method("POST"))
		.and(path("/token"))
		.and(body_string_contains("grant_type=refresh_token"))
		.and(body_string_contains("refresh_token=refresh-1-canary"))
		.and(body_string_contains("client_secret=rotated-client-canary"))
		.respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"access_token":"access-2-canary","token_type":"Bearer","refresh_token":"refresh-2-canary","expires_in":3600})))
		.expect(1)
		.mount(&server)
		.await;
	let response = f
		.call(
			"/v1/connections/refresh",
			ConnectionOperation::Refresh {
				version: "1".into(),
				sub: f.sub(),
			},
			"",
		)
		.await;
	assert_eq!(response.status(), 200);
	let receipt = json(response).await;
	assert_eq!(receipt["version"], "2");
	assert_eq!(receipt["scope"], "read");
	for (version, token) in [
		("1", "Bearer access-1-canary"),
		("2", "Bearer access-2-canary"),
	] {
		Mock::given(method("POST"))
			.and(path("/mcp"))
			.and(header("authorization", token))
			.respond_with(ResponseTemplate::new(200).set_body_string("ok"))
			.expect(1)
			.mount(&server)
			.await;
		let response = f
			.call(
				"/v1/forward/mcp",
				f.forward(version, "https://mcp.example", "/"),
				"{}",
			)
			.await;
		assert_eq!(response.status(), 200);
		assert_eq!(
			to_bytes(response.into_body(), 32).await.unwrap().as_ref(),
			b"ok"
		);
	}
	// Simulate a lost DB re-pin. Replaying V=1 must reuse version 2 without
	// another token request (the wiremock refresh expectation remains exactly 1).
	let response = f
		.call(
			"/v1/connections/refresh",
			ConnectionOperation::Refresh {
				version: "1".into(),
				sub: f.sub(),
			},
			"",
		)
		.await;
	assert_eq!(response.status(), 200);
	assert_eq!(json(response).await["version"], "2");
	Mock::given(method("POST"))
		.and(path("/revoke"))
		.and(body_string_contains("token=refresh-2-canary"))
		.and(body_string_contains("client_secret=rotated-client-canary"))
		.respond_with(ResponseTemplate::new(200))
		.expect(1)
		.mount(&server)
		.await;
	assert_eq!(
		f.call(
			"/v1/connections/revoke",
			ConnectionOperation::Revoke {
				version: "2".into()
			},
			""
		)
		.await
		.status(),
		204
	);
	let logs = f.logs.0.lock().unwrap().join("\n");
	for canary in [
		"access-1-canary",
		"refresh-1-canary",
		"client-canary",
		"code-canary",
		"verifier-canary",
	] {
		assert!(!logs.contains(canary))
	}
}
#[tokio::test]
async fn sealed_bindings_refuse_a_forged_exchange_or_forward_destination() {
	let server = MockServer::start().await;
	let f = Fixture::new(&server.uri());
	let payload = sealed::seal(Payload::DynamicClient {
		issuer: server.uri(),
		client_id: "registered".into(),
		redirect_uri: "https://aidash.example/callback".into(),
		client_secret: "canary".into(),
	})
	.unwrap();
	f.store
		.0
		.lock()
		.unwrap()
		.insert((f.resource(f.client), "1".into()), payload.to_vec());
	for alteration in 0..3 {
		let mut c = f.exchange(&server.uri());
		if let ConnectionOperation::Exchange {
			issuer,
			client_id,
			client,
			redirect_uri,
			..
		} = &mut c
		{
			client.as_mut().unwrap().kind = ClientKind::Dynamic;
			match alteration {
				0 => *issuer = "https://evil.example".into(),
				1 => *client_id = "other".into(),
				_ => *redirect_uri = "https://evil.example/callback".into(),
			}
		}
		let response = f
			.call(
				"/v1/connections/exchange",
				c,
				r#"{"code":"code","verifier":"verifier"}"#,
			)
			.await;
		assert_eq!(response.status(), 403);
		assert_eq!(json(response).await["error"], DESTINATION_REFUSED);
	}
	let payload = sealed::seal(Payload::Connection {
		issuer: server.uri(),
		resource: "https://mcp.example/mcp".into(),
		client_id: "public".into(),
		client: None,
		redirect_uri: None,
		access_token: "canary".into(),
		refresh_token: None,
		expires_at: None,
		scope: None,
		previous: None,
	})
	.unwrap();
	f.store
		.0
		.lock()
		.unwrap()
		.insert((f.resource(f.connection), "1".into()), payload.to_vec());
	for (origin, rest) in [
		("https://evil.example", "mcp"),
		("https://mcp.example", "sibling"),
		("https://mcp.example", "mcp-evil"),
		("https://mcp.example", "mcp/%2e%2e/sibling"),
	] {
		assert_eq!(
			f.call(
				&format!("/v1/forward/{rest}"),
				f.forward("1", origin, "/"),
				"{}"
			)
			.await
			.status(),
			403
		)
	}
	assert!(server.received_requests().await.unwrap().is_empty());
}
#[tokio::test]
async fn invalid_grant_requires_reconnection_and_public_clients_read_no_client_secret() {
	let server = MockServer::start().await;
	metadata(&server).await;
	let f = Fixture::new(&server.uri());
	let payload = sealed::seal(Payload::Connection {
		issuer: server.uri(),
		resource: "https://mcp.example/mcp".into(),
		client_id: "public".into(),
		client: None,
		redirect_uri: None,
		access_token: "access".into(),
		refresh_token: Some("refresh".into()),
		expires_at: None,
		scope: None,
		previous: None,
	})
	.unwrap();
	f.store
		.0
		.lock()
		.unwrap()
		.insert((f.resource(f.connection), "1".into()), payload.to_vec());
	Mock::given(method("POST"))
		.and(path("/token"))
		.and(body_string_contains("client_id=public"))
		.respond_with(
			ResponseTemplate::new(400).set_body_json(serde_json::json!({"error":"invalid_grant"})),
		)
		.expect(1)
		.mount(&server)
		.await;
	let response = f
		.call(
			"/v1/connections/refresh",
			ConnectionOperation::Refresh {
				version: "1".into(),
				sub: f.sub(),
			},
			"",
		)
		.await;
	assert_eq!(response.status(), 401);
	assert_eq!(
		response.headers()["x-aidash-broker-error"],
		"connection_required"
	);
	assert_eq!(
		json(response).await,
		serde_json::json!({"error":"connection_required"})
	);
	assert_eq!(f.store.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn refresh_refuses_a_pin_superseded_by_an_unrelated_exchange() {
	let server = MockServer::start().await;
	let f = Fixture::new(&server.uri());
	for version in ["1", "2"] {
		let payload = sealed::seal(Payload::Connection {
			issuer: server.uri(),
			resource: "https://mcp.example/mcp".into(),
			client_id: "public".into(),
			client: None,
			redirect_uri: None,
			access_token: "canary".into(),
			refresh_token: Some("refresh".into()),
			expires_at: None,
			scope: None,
			previous: None,
		})
		.unwrap();
		f.store
			.0
			.lock()
			.unwrap()
			.insert((f.resource(f.connection), version.into()), payload.to_vec());
	}
	let response = f
		.call(
			"/v1/connections/refresh",
			ConnectionOperation::Refresh {
				version: "1".into(),
				sub: f.sub(),
			},
			"",
		)
		.await;
	assert_eq!(response.status(), 401);
	assert_eq!(json(response).await["error"], "connection_required");
	assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_replaced_client_bound_to_another_issuer_cannot_receive_existing_grant_tokens() {
	let server = MockServer::start().await;
	let f = Fixture::new(&server.uri());
	let connection = sealed::seal(Payload::Connection {
		issuer: server.uri(),
		resource: "https://mcp.example/mcp".into(),
		client_id: "registered".into(),
		client: Some(ClientSecretRef {
			kind: ClientKind::Registered,
			id: f.client,
		}),
		redirect_uri: Some("https://aidash.example/callback".into()),
		access_token: "access-canary".into(),
		refresh_token: Some("refresh-canary".into()),
		expires_at: None,
		scope: None,
		previous: None,
	})
	.unwrap();
	let client = sealed::seal(Payload::RegisteredClient {
		issuer: "https://evil.example".into(),
		client_id: "registered".into(),
		client_secret: "evil-secret".into(),
	})
	.unwrap();
	{
		let mut data = f.store.0.lock().unwrap();
		data.insert((f.resource(f.connection), "1".into()), connection.to_vec());
		data.insert((f.resource(f.client), "2".into()), client.to_vec());
	}
	let mut receipt: serde_json::Value = serde_json::from_slice(&connection).unwrap();
	receipt["previous"] = "1".into();
	f.store.0.lock().unwrap().insert(
		(f.resource(f.connection), "2".into()),
		serde_json::to_vec(&receipt).unwrap(),
	);

	for operation in [
		ConnectionOperation::Refresh {
			version: "1".into(),
			sub: f.sub(),
		},
		ConnectionOperation::Refresh {
			version: "2".into(),
			sub: f.sub(),
		},
		ConnectionOperation::Revoke {
			version: "2".into(),
		},
	] {
		let path = if matches!(operation, ConnectionOperation::Refresh { .. }) {
			"/v1/connections/refresh"
		} else {
			"/v1/connections/revoke"
		};
		let response = f.call(path, operation, "").await;
		assert_eq!(response.status(), 403);
		assert_eq!(json(response).await["error"], DESTINATION_REFUSED);
	}
	assert!(server.received_requests().await.unwrap().is_empty());
}

#[rstest::rstest]
#[case::discovery_failure(false)]
#[case::revocation_failure(true)]
#[tokio::test]
async fn revocation_reports_upstream_failure(#[case] discovery_succeeds: bool) {
	let server = MockServer::start().await;
	if discovery_succeeds {
		metadata(&server).await;
		Mock::given(method("POST"))
			.and(path("/revoke"))
			.respond_with(ResponseTemplate::new(500))
			.expect(1)
			.mount(&server)
			.await;
	}
	let f = Fixture::new(&server.uri());
	let payload = sealed::seal(Payload::Connection {
		issuer: server.uri(),
		resource: "https://mcp.example/mcp".into(),
		client_id: "public".into(),
		client: None,
		redirect_uri: None,
		access_token: "token".into(),
		refresh_token: None,
		expires_at: None,
		scope: None,
		previous: None,
	})
	.unwrap();
	f.store
		.0
		.lock()
		.unwrap()
		.insert((f.resource(f.connection), "1".into()), payload.to_vec());
	let response = f
		.call(
			"/v1/connections/revoke",
			ConnectionOperation::Revoke {
				version: "1".into(),
			},
			"{}",
		)
		.await;
	assert!(!response.status().is_success());
}
