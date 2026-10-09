//! Keep both tracing captures in this single-test process. Parallel tests with
//! scoped subscribers can race tracing's process-wide callsite interest cache.
use aidash_application::{
	Error, Result,
	provider_access::{Access, Context, ProviderAccess, Source, TokenIssuer},
};
use aidash_broker::{Audit, AuditSink, CloudLogging};
use aidash_capability::{Claims, InMemorySigner, Operation, SignError, TokenSigner};
use aidash_domain::provider_credentials::{Provider, ProviderCredential, State};
use aidash_integrations::capability::issuer::{CapabilityIssuer, WorkerConfiguration};
use axum::{
	Router,
	extract::{Json, State as HttpState},
	http::{HeaderMap, StatusCode, header},
	routing::post,
};
use base64::Engine;
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use std::sync::{
	Arc, Mutex,
	atomic::{AtomicUsize, Ordering},
};
use uuid::Uuid;

const CANARY: &str = "canary-key-must-not-appear-9e74bd";

struct Writer(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Writer {
	fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
		self.0.lock().unwrap().extend_from_slice(bytes);
		Ok(bytes.len())
	}
	fn flush(&mut self) -> std::io::Result<()> {
		Ok(())
	}
}

struct Signer {
	inner: InMemorySigner,
	claims: Mutex<Vec<Claims>>,
}
#[async_trait::async_trait]
impl TokenSigner for Signer {
	fn kid(&self) -> &str {
		self.inner.kid()
	}
	async fn sign(&self, input: &[u8]) -> std::result::Result<[u8; 64], SignError> {
		let payload = std::str::from_utf8(input)
			.unwrap()
			.split('.')
			.nth(1)
			.unwrap();
		let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
			.decode(payload)
			.unwrap();
		self.claims
			.lock()
			.unwrap()
			.push(serde_json::from_slice(&payload).unwrap());
		self.inner.sign(input).await
	}
}

/// Supply the validated metadata to the real worker issuer. Tenant ownership,
/// admission pinning and binding resolution remain exercised by the Cloud suite.
struct AccessFixture {
	issuer: CapabilityIssuer,
	credential: ProviderCredential,
}
#[async_trait::async_trait]
impl ProviderAccess for AccessFixture {
	async fn resolve(&self, context: &Context, endpoint: &str, source: &Source) -> Result<Access> {
		assert_eq!(endpoint, Provider::Openrouter.base_url());
		assert!(matches!(source, Source::Tenant { provider } if provider == "openrouter"));
		self.issuer.mint(context, &self.credential).await
	}
}

#[derive(Default)]
struct Attempts {
	calls: AtomicUsize,
	bearer: Mutex<Option<SecretString>>,
}
async fn expired(
	HttpState(state): HttpState<Arc<Attempts>>,
	headers: HeaderMap,
	Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
	state.calls.fetch_add(1, Ordering::SeqCst);
	assert!(body["messages"].to_string().contains(CANARY));
	let bearer = headers[header::AUTHORIZATION]
		.to_str()
		.unwrap()
		.strip_prefix("Bearer ")
		.unwrap();
	*state.bearer.lock().unwrap() = Some(bearer.to_owned().into());
	(
		StatusCode::UNAUTHORIZED,
		Json(json!({"error":{"code":"capability_expired","message":CANARY}})),
	)
}

#[tokio::test]
async fn worker_mint_and_broker_audit_are_correlated_and_never_log_secrets() {
	let output = Arc::new(Mutex::new(Vec::new()));
	let capture = output.clone();
	// Permanent global installation before any mint/audit callsite is used. This
	// binary intentionally has one test, so there are no competing dispatchers.
	tracing::subscriber::set_global_default(
		tracing_subscriber::fmt()
			.json()
			.with_max_level(tracing::Level::TRACE)
			.with_writer(move || Writer(capture.clone()))
			.finish(),
	)
	.unwrap();
	let attempts = Arc::new(Attempts::default());
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/api/v1", listener.local_addr().unwrap());
	let router = Router::new()
		.route("/api/v1/chat/completions", post(expired))
		.with_state(attempts.clone());
	let server = tokio::spawn(async move {
		axum::serve(listener, router).await.unwrap();
	});
	let signer = Arc::new(Signer {
		inner: InMemorySigner::new("kms/1".into(), [7; 32]),
		claims: Mutex::new(vec![]),
	});
	let id = Uuid::now_v7();
	let resource = format!("projects/byok/secrets/aidash-environment-cred-{id}");
	let access = AccessFixture {
		issuer: CapabilityIssuer::new(
			WorkerConfiguration {
				endpoint,
				issuer: "aidash-worker".into(),
				audience: "environment".into(),
				kid: signer.kid().into(),
			},
			"byok".into(),
			signer.clone(),
		)
		.unwrap(),
		credential: ProviderCredential {
			id,
			tenant: "tenant-a".into(),
			provider: Provider::Openrouter,
			base_url: Provider::Openrouter.base_url().into(),
			pinned_version: Some(format!("{resource}/versions/1")),
			secret_resource: resource,
			fingerprint: "fixture".into(),
			last4: "test".into(),
			state: State::Active,
			created_at: chrono::Utc::now(),
			rotated_at: None,
			revoked_at: None,
			revision: 1,
		},
	};
	let context = Context {
		tenant: "tenant-a".into(),
		run: Some(Uuid::now_v7()),
		provider_credential_id: Some(id),
		..Default::default()
	};
	let provider = aidash_integrations::inference::provider(
		reqwest::Client::new(),
		aidash_domain::model::ModelConfig {
			provider: "openrouter".into(),
			model_id: "author/model".into(),
			endpoint: Provider::Openrouter.base_url().into(),
			credential_env: None,
			provider_credential: Some("openrouter".into()),
			request_timeout_secs: Some(5),
			reasoning_effort: None,
			context_window: 32768,
			max_output_tokens: Some(10),
			modalities: vec!["text".into()],
			media_routes: vec![],
			cost: json!({}),
		},
		Arc::new(access),
		context,
	)
	.unwrap();
	let error = provider
		.infer(aidash_domain::provider::ModelRequest {
			instructions: CANARY.into(),
			context: json!({}),
			tools: vec![],
			max_output_tokens: 10,
			content_parts: vec![],
		})
		.await
		.unwrap_err();
	assert!(matches!(error, Error::Invalid(_)));
	assert!(error.to_string().contains("capability_expired"));
	assert!(!error.to_string().contains(CANARY));
	assert_eq!(attempts.calls.load(Ordering::SeqCst), 1);
	let claims = signer.claims.lock().unwrap().clone();
	assert_eq!(claims.len(), 1);
	let claim = &claims[0];
	CloudLogging.record(&Audit {
		jti: claim.jti,
		subject: claim.sub.clone(),
		tenant: claim.tenant.clone(),
		credential: claim.credential,
		version: claim.version.clone(),
		provider: claim.provider.clone(),
		model: claim.model.clone(),
		operation: Operation::Chat,
		status: 401,
		latency_ms: 1,
		usage: Some(json!({"total_tokens":6})),
	});
	server.abort();
	let text = String::from_utf8(output.lock().unwrap().clone()).unwrap();
	let entries: Vec<Value> = text
		.lines()
		.map(|line| serde_json::from_str(line).unwrap())
		.collect();
	for message in ["Capability Token minted", "provider_call"] {
		let matching: Vec<_> = entries
			.iter()
			.filter(|entry| entry["fields"]["message"] == message)
			.collect();
		assert_eq!(
			matching.len(),
			1,
			"must capture exactly one {message} event"
		);
		assert_eq!(matching[0]["fields"]["jti"], claim.jti.to_string());
		assert!(
			matching[0]["fields"]["subject"]
				.as_str()
				.unwrap()
				.contains("Run")
		);
	}
	assert!(!text.contains(CANARY));
	assert!(!text.contains("Bearer "));
	assert!(
		!text.contains(
			attempts
				.bearer
				.lock()
				.unwrap()
				.as_ref()
				.unwrap()
				.expose_secret()
		)
	);
}
