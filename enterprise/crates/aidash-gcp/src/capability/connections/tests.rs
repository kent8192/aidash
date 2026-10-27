use super::*;
use aidash_capability::{
	InMemorySigner, PublicKeys,
	tenant_secret::{Attach, Destination},
};
use axum::{
	Json, Router,
	body::Bytes,
	http::{HeaderMap, StatusCode, Uri},
};
use std::{collections::BTreeMap, sync::Mutex};
#[tokio::test]
async fn application_adapter_sends_connection_tokens_and_returns_only_receipts() {
	let signer = Arc::new(InMemorySigner::new("key".into(), [3; 32]));
	let mut keys = PublicKeys::default();
	keys.insert("key".into(), signer.public_key());
	let keys = Arc::new(keys);
	let paths = Arc::new(Mutex::new(Vec::new()));
	let seen = paths.clone();
	let app = Router::new().fallback(move |uri: Uri, headers: HeaderMap, body: Bytes| {
		let keys = keys.clone();
		let seen = seen.clone();
		async move {
			let now = SystemTime::now()
				.duration_since(UNIX_EPOCH)
				.unwrap()
				.as_secs();
			let token = headers["x-aidash-secret-call"].to_str().unwrap();
			let issuers = BTreeMap::from([("key".into(), "server".into())]);
			let c = keys
				.verify_connection(token, &issuers, "dev-tsec", now)
				.unwrap();
			assert_eq!(c.tenant, "tenant");
			assert!(
				keys.verify_secret(token, &issuers, "dev-tsec", now)
					.is_err()
			);
			seen.lock().unwrap().push(uri.path().to_owned());
			if uri.path().ends_with("exchange") {
				assert!(matches!(
					c.operation,
					ConnectionOperation::Exchange { client: None, .. }
				));
				let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
				assert_eq!(body["code"], "code-canary");
				assert_eq!(body["verifier"], "verifier-canary");
			}
			if uri.path().ends_with("revoke") {
				return (StatusCode::NO_CONTENT, Json(serde_json::Value::Null));
			}
			(
				StatusCode::OK,
				Json(
					serde_json::json!({"version":"5","expires_in":3600,"scope":"read","refresh":true}),
				),
			)
		}
	});
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let url = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
	let adapter = ConnectionCallsAdapter::with_signer(
		&url,
		SecretIssuerConfiguration {
			issuer: "server".into(),
			audience: "dev-tsec".into(),
			kid: "key".into(),
		},
		signer,
	)
	.unwrap();
	let id = Uuid::now_v7();
	let receipt = adapter
		.exchange(
			"tenant",
			id,
			ExchangeGrant {
				issuer: "https://auth.example".into(),
				resource: "https://mcp.example/mcp".into(),
				client_id: "public".into(),
				client: None,
				redirect_uri: "https://aidash.example/callback".into(),
			},
			"code-canary",
			&"verifier-canary".into(),
		)
		.await
		.unwrap();
	assert_eq!(receipt.version, "5");
	assert_eq!(
		adapter.refresh("tenant", id, "5").await.unwrap().version,
		"5"
	);
	adapter.revoke("tenant", id, "5").await.unwrap();
	let route = adapter
		.route(
			"tenant",
			id,
			"5",
			SecretCall {
				destination: Destination {
					origin: "https://mcp.example".into(),
					path_prefix: "/".into(),
				},
				methods: vec!["POST".into()],
				attach: Attach::Bearer,
				subject: SecretSubject::Run {
					run: "run".into(),
					call: Uuid::new_v4(),
				},
				max_response_bytes: None,
			},
		)
		.await
		.unwrap();
	assert_eq!(route.base.as_str(), format!("{url}/v1/forward"));
	assert_eq!(paths.lock().unwrap().len(), 3);
	server.abort();
}

struct Tokens;
#[async_trait]
impl AccessTokenSource for Tokens {
	async fn token(&self) -> std::result::Result<SecretString, SignError> {
		Ok("gcp-canary".into())
	}
}
#[tokio::test]
async fn broker_store_resolves_latest_with_checksum_and_writes_only_oauth_connection_versions() {
	let id = Uuid::now_v7();
	let resource = format!("projects/123/secrets/aidash-dev-oauth-{id}");
	let payload = sealed::seal(sealed::Payload::Connection {
		issuer: "https://auth.example".into(),
		resource: "https://mcp.example/mcp".into(),
		client_id: "public".into(),
		client: None,
		redirect_uri: None,
		access_token: "access-canary".into(),
		refresh_token: Some("refresh-canary".into()),
		expires_at: None,
		scope: None,
		previous: Some("1".into()),
	})
	.unwrap();
	let remote = resource.clone();
	let data = payload.to_vec();
	let calls = Arc::new(Mutex::new(Vec::new()));
	let seen = calls.clone();
	let app = Router::new().fallback(move |uri: Uri, headers: HeaderMap, body: Bytes| {
		let resource = remote.clone();
		let data = data.clone();
		let seen = seen.clone();
		async move {
			assert_eq!(headers["authorization"], "Bearer gcp-canary");
			seen.lock().unwrap().push(uri.path().to_owned());
			let response = if uri.path().ends_with(":addVersion") {
				let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
				let bytes = STANDARD
					.decode(v["payload"]["data"].as_str().unwrap())
					.unwrap();
				assert!(matches!(
					sealed::open(&bytes),
					Ok(sealed::Payload::Connection { .. })
				));
				serde_json::json!({"name":format!("{resource}/versions/3")})
			} else {
				serde_json::json!({
					"name":format!("{resource}/versions/2"),
					"payload":{
						"data":STANDARD.encode(&data),
						"dataCrc32c":crc32c::crc32c(&data).to_string()
					}
				})
			};
			Json(response)
		}
	});
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let url = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
	let mut store =
		SecretManagerConnectionStore::new("123", "aidash-dev-tsec-", Arc::new(Tokens)).unwrap();
	store.source.base = url;
	let (version, bytes) = store.latest(&resource).await.unwrap();
	assert_eq!(version, "2");
	assert_eq!(&*bytes, &*payload);
	assert_eq!(store.add_version(&resource, &payload).await.unwrap(), "3");
	assert!(store.access(&resource, "latest").await.is_err());
	assert!(
		store
			.latest(&resource.replace("-oauth-", "-cred-"))
			.await
			.is_err()
	);
	let client = sealed::seal(sealed::Payload::RegisteredClient {
		issuer: "https://auth.example".into(),
		client_id: "client".into(),
		client_secret: "canary".into(),
	})
	.unwrap();
	assert!(store.add_version(&resource, &client).await.is_err());
	assert_eq!(calls.lock().unwrap().len(), 2);
	server.abort();
}

#[tokio::test]
async fn broker_faults_keep_operator_correlation_while_invalid_grant_is_terminal() {
	for (code, status) in [
		("broker_rejected", 401),
		("broker_unavailable", 503),
		("connection_required", 401),
		("upstream_unreachable", 502),
	] {
		let app = Router::new().fallback(move || async move {
			(
				StatusCode::from_u16(status).unwrap(),
				[
					("x-aidash-broker-error", code),
					("x-aidash-correlation", "correlation-canary"),
				],
				Json(serde_json::json!({"error":code})),
			)
		});
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let url = format!("http://{}", listener.local_addr().unwrap());
		let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
		let adapter = ConnectionCallsAdapter::with_signer(
			&url,
			SecretIssuerConfiguration {
				issuer: "server".into(),
				audience: "dev-tsec".into(),
				kid: "key".into(),
			},
			Arc::new(InMemorySigner::new("key".into(), [4; 32])),
		)
		.unwrap();
		let failure = adapter
			.refresh("tenant", Uuid::now_v7(), "1")
			.await
			.err()
			.unwrap();
		match (code, failure) {
			(
				"broker_rejected",
				RefreshFailure::Unavailable(Error::BrokerRejected { correlation }),
			)
			| (
				"broker_unavailable",
				RefreshFailure::Unavailable(Error::BrokerUnavailable { correlation }),
			) => assert_eq!(correlation.as_deref(), Some("correlation-canary")),
			("connection_required", RefreshFailure::Terminal) => {}
			("upstream_unreachable", RefreshFailure::Unavailable(Error::External(c))) => {
				assert_eq!(c, "connection_unavailable")
			}
			(code, other) => panic!("unexpected classification for {code}: {other:?}"),
		}
		server.abort();
	}
}
