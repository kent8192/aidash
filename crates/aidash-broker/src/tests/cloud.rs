//! Acceptance through the real Cloud ProviderAccess, issuer, HTTP broker and fakes.
use super::*;
use aidash_application::{
	provider_access::{
		Context, EnvironmentAccess, Inference, MaintenancePurpose, Operation as CallOperation,
		ProviderAccess, Source as AccessSource, TenantAccess,
	},
	provider_credentials::{Repository, Scope},
};
use aidash_domain::provider_credentials::{
	Binding, Provider, ProviderCredential, State as CredentialState,
};
use aidash_integrations::capability::issuer::{CapabilityIssuer, WorkerConfiguration};
use base64::Engine;
use uuid::Uuid;

#[derive(Clone, Default)]
struct Repo {
	rows: Arc<Mutex<HashMap<Uuid, ProviderCredential>>>,
	bindings: Arc<Mutex<Vec<Binding>>>,
	begins: Arc<AtomicUsize>,
}
#[async_trait]
impl Repository for Repo {
	async fn begin(&self, _: &str) -> aidash_application::Result<Box<dyn Scope>> {
		self.begins.fetch_add(1, Ordering::SeqCst);
		Ok(Box::new(self.clone()))
	}
	async fn reconciliation_candidates(
		&self,
		_after: Option<Uuid>,
		_limit: usize,
	) -> aidash_application::Result<Vec<ProviderCredential>> {
		Ok(vec![])
	}
}
#[async_trait]
impl Scope for Repo {
	async fn get(&mut self, id: Uuid) -> aidash_application::Result<ProviderCredential> {
		self.rows
			.lock()
			.unwrap()
			.get(&id)
			.cloned()
			.ok_or_else(|| aidash_application::Error::NotFound("Provider Credential".into()))
	}
	async fn bindings(&mut self) -> aidash_application::Result<Vec<Binding>> {
		Ok(self.bindings.lock().unwrap().clone())
	}
	async fn commit(self: Box<Self>) -> aidash_application::Result<()> {
		Ok(())
	}
	async fn list(
		&mut self,
		_: usize,
		_: usize,
	) -> aidash_application::Result<Vec<ProviderCredential>> {
		unreachable!()
	}
	async fn count(&mut self) -> aidash_application::Result<usize> {
		unreachable!()
	}
	async fn insert(&mut self, _: &ProviderCredential) -> aidash_application::Result<()> {
		unreachable!()
	}
	async fn save(
		&mut self,
		_: &ProviderCredential,
		_: &str,
		_: &str,
	) -> aidash_application::Result<()> {
		unreachable!()
	}
	async fn bind(&mut self, _: &Binding, _: &str) -> aidash_application::Result<()> {
		unreachable!()
	}
}
struct Signer {
	inner: InMemorySigner,
	claims: Mutex<Vec<Claims>>,
	unavailable: AtomicBool,
}
#[async_trait]
impl TokenSigner for Signer {
	fn kid(&self) -> &str {
		self.inner.kid()
	}
	async fn sign(&self, input: &[u8]) -> Result<[u8; 64], aidash_capability::SignError> {
		let payload = std::str::from_utf8(input)
			.unwrap()
			.split('.')
			.nth(1)
			.unwrap();
		let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
			.decode(payload)
			.unwrap();
		self.claims
			.lock()
			.unwrap()
			.push(serde_json::from_slice(&bytes).unwrap());
		if self.unavailable.load(Ordering::SeqCst) {
			return Err(aidash_capability::SignError);
		}
		self.inner.sign(input).await
	}
}
struct Env(Mutex<String>);
impl aidash_application::ports::Credentials for Env {
	fn resolve(&self, _: &str) -> aidash_application::Result<String> {
		Ok(self.0.lock().unwrap().clone())
	}
}
struct Cloud {
	fixture: Fixture,
	access: Arc<TenantAccess>,
	repo: Repo,
	signer: Arc<Signer>,
	env: Arc<Env>,
	context: Context,
	broker_calls: Arc<AtomicUsize>,
	server: tokio::task::JoinHandle<()>,
}
impl Drop for Cloud {
	fn drop(&mut self) {
		self.server.abort();
	}
}
fn row(id: Uuid, tenant: &str, version: &str) -> ProviderCredential {
	let resource = format!("projects/byok/secrets/aidash-environment-cred-{id}");
	ProviderCredential {
		id,
		tenant: tenant.into(),
		provider: Provider::Openrouter,
		base_url: Provider::Openrouter.base_url().into(),
		secret_resource: resource.clone(),
		pinned_version: Some(format!("{resource}/versions/{version}")),
		fingerprint: "fixture".into(),
		last4: "test".into(),
		state: CredentialState::Active,
		created_at: chrono::Utc::now(),
		rotated_at: None,
		revoked_at: None,
		revision: 1,
	}
}
impl Cloud {
	async fn new(expired_broker: bool) -> Self {
		Self::with_deadline(expired_broker, Duration::from_secs(5)).await
	}
	async fn with_deadline(expired_broker: bool, deadline: Duration) -> Self {
		Self::with_limits(expired_broker, deadline, 100, 100.0).await
	}
	async fn with_limits(
		expired_broker: bool,
		deadline: Duration,
		burst: u32,
		requests_per_second: f64,
	) -> Self {
		let fixture = Fixture::new().await;
		let signer = Arc::new(Signer {
			inner: InMemorySigner::new("kms/1".into(), [7; 32]),
			claims: Mutex::new(vec![]),
			unavailable: AtomicBool::new(false),
		});
		let mut keys = PublicKeys::default();
		keys.insert(signer.kid().into(), signer.inner.public_key());
		let mut configuration = config();
		configuration.secret_prefix = "aidash-environment-cred-".into();
		configuration.inference_deadline = deadline;
		configuration.burst = burst;
		configuration.requests_per_second = requests_per_second;
		let mut broker = Broker::new(
			configuration,
			keys,
			fixture.source.clone(),
			fixture.logs.clone(),
		)
		.unwrap();
		broker.catalog = fixture.broker.catalog.clone();
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let endpoint = format!("http://{}/api/v1", listener.local_addr().unwrap());
		let router = if expired_broker {
			Router::new().fallback(any(|| async {
				(
					StatusCode::UNAUTHORIZED,
					Json(json!({"error":{"code":"capability_expired","message":CANARY}})),
				)
			}))
		} else {
			Arc::new(broker).router()
		};
		let broker_calls = Arc::new(AtomicUsize::new(0));
		let calls = broker_calls.clone();
		let router = router.layer(axum::middleware::from_fn(
			move |request: axum::extract::Request, next: axum::middleware::Next| {
				let calls = calls.clone();
				async move {
					calls.fetch_add(1, Ordering::SeqCst);
					next.run(request).await
				}
			},
		));
		let server = tokio::spawn(async move {
			axum::serve(listener, router).await.unwrap();
		});
		let repo = Repo::default();
		let id = Uuid::now_v7();
		repo.rows
			.lock()
			.unwrap()
			.insert(id, row(id, "tenant-a", "1"));
		let env = Arc::new(Env(Mutex::new(CANARY.into())));
		let issuer = CapabilityIssuer::new(
			WorkerConfiguration {
				endpoint,
				issuer: "worker".into(),
				audience: "environment".into(),
				kid: signer.kid().into(),
			},
			"byok".into(),
			signer.clone(),
		)
		.unwrap();
		let access = Arc::new(TenantAccess {
			environment: EnvironmentAccess {
				credentials: env.clone(),
			},
			repository: Arc::new(repo.clone()),
			reader: None,
			issuer: Some(Arc::new(issuer)),
		});
		let context = Context {
			tenant: "tenant-a".into(),
			run: Some(Uuid::now_v7()),
			provider_credential_id: Some(id),
			..Default::default()
		};
		Self {
			fixture,
			access,
			repo,
			signer,
			env,
			context,
			broker_calls,
			server,
		}
	}
	fn config(&self) -> aidash_domain::model::ModelConfig {
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
			modalities: vec!["text".into(), "image".into()],
			media_routes: vec![],
			cost: json!({}),
			projection_versions: vec![],
		}
	}
	fn request() -> aidash_domain::provider::ModelRequest {
		aidash_domain::provider::ModelRequest {
			instructions: "test".into(),
			context: json!({}).into(),
			tools: vec![],
			max_output_tokens: 10,
			content_parts: vec![],
			cache_scope: None,
		}
	}
	fn scoped(&self, operation: CallOperation) -> Context {
		let mut context = self.context.clone();
		context.inference = Some(Inference {
			model: "author/model".into(),
			operations: vec![operation],
			max_output_tokens: 10,
		});
		context
	}
}

#[tokio::test]
async fn cloud_chat_accepts_the_full_raw_media_allowance_after_base64_encoding() {
	// Debug builds must serialize and parse the full 8 MiB allowance several times.
	// The two concurrent discovery requests and chat must fit without a refill.
	let c = Cloud::with_limits(false, Duration::from_secs(60), 3, 0.001).await;
	let mut config = c.config();
	config.context_window = 131_072;
	config.request_timeout_secs = Some(60);
	config.media_routes = vec![aidash_domain::model::MediaRouteEvidence {
		tag: "vendor/route".into(),
		formats: vec!["image/png".into()],
		source: "fixture".into(),
		verified_at: chrono::Utc::now() - chrono::Duration::seconds(10),
		expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
	}];
	let provider = aidash_integrations::inference::provider(
		reqwest::Client::new(),
		config,
		c.access.clone(),
		c.context.clone(),
	)
	.unwrap();
	let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
	bytes.resize(8 * 1024 * 1024, 0);
	let mut request = Cloud::request();
	request
		.content_parts
		.push(aidash_domain::provider::ContentPart::Image {
			media_type: "image/png".into(),
			bytes,
		});
	assert!(aidash_domain::provider::ModelRequest::media_within_limits(
		&request.content_parts
	));
	let size = request.input_body().to_string().len();
	assert!(size > REQUEST_LIMIT && size < CHAT_REQUEST_LIMIT);
	assert_eq!(provider.infer(request).await.unwrap().text, "ok");
	assert_eq!(c.broker_calls.load(Ordering::SeqCst), 3);
	assert!(
		c.fixture
			.provider
			.paths
			.lock()
			.unwrap()
			.iter()
			.any(|p| p.ends_with("/chat/completions"))
	);
}

#[tokio::test]
async fn transient_signing_failure_is_external_and_recovers_before_any_broker_call() {
	let c = Cloud::new(false).await;
	let provider = aidash_integrations::inference::provider(
		reqwest::Client::new(),
		c.config(),
		c.access.clone(),
		c.context.clone(),
	)
	.unwrap();
	c.signer.unavailable.store(true, Ordering::SeqCst);
	let error = provider.infer(Cloud::request()).await.unwrap_err();
	assert!(matches!(error, aidash_application::Error::External(_)));
	assert_eq!(error.to_string(), "Capability Token signing unavailable");
	assert_eq!(c.signer.claims.lock().unwrap().len(), 1);
	assert_eq!(c.broker_calls.load(Ordering::SeqCst), 0);
	assert_eq!(c.fixture.source.reads.load(Ordering::SeqCst), 0);
	c.signer.unavailable.store(false, Ordering::SeqCst);
	assert_eq!(provider.infer(Cloud::request()).await.unwrap().text, "ok");
	assert_eq!(c.signer.claims.lock().unwrap().len(), 2);
	assert_eq!(c.broker_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cloud_mint_routes_chat_media_discovery_embeddings_and_early_streaming() {
	let c = Cloud::new(false).await;
	let client = reqwest::Client::new();
	let provider = aidash_integrations::inference::provider(
		client.clone(),
		c.config(),
		c.access.clone(),
		c.context.clone(),
	)
	.unwrap();
	assert_eq!(provider.infer(Cloud::request()).await.unwrap().text, "ok");
	let mut config = c.config();
	config.media_routes = vec![aidash_domain::model::MediaRouteEvidence {
		tag: "vendor/route".into(),
		formats: vec!["image/png".into()],
		source: "fixture".into(),
		verified_at: chrono::Utc::now() - chrono::Duration::seconds(10),
		expires_at: chrono::Utc::now() + chrono::Duration::minutes(10),
	}];
	let provider = aidash_integrations::inference::provider(
		client.clone(),
		config,
		c.access.clone(),
		c.context.clone(),
	)
	.unwrap();
	let mut request = Cloud::request();
	request
		.content_parts
		.push(aidash_domain::provider::ContentPart::Image {
			media_type: "image/png".into(),
			bytes: b"\x89PNG\r\n\x1a\n".to_vec(),
		});
	assert_eq!(provider.infer(request).await.unwrap().text, "ok");
	let embedding = aidash_integrations::semantic::embed(
		c.access.as_ref(),
		&c.context,
		&client,
		&aidash_domain::semantic::EmbeddingConfig {
			provider: "openrouter".into(),
			endpoint: Provider::Openrouter.base_url().into(),
			credential_env: None,
			provider_credential: Some("openrouter".into()),
			model: "author/model".into(),
			model_version: "1".into(),
			dimensions: 2,
		},
		"test",
	)
	.await
	.unwrap();
	assert_eq!(embedding.vector, vec![1.0, 0.0]);
	let access = c
		.access
		.resolve(
			&c.scoped(CallOperation::Chat),
			Provider::Openrouter.base_url(),
			&AccessSource::Tenant {
				provider: "openrouter".into(),
			},
		)
		.await
		.unwrap();
	let mut body = chat();
	body["stream"] = json!(true);
	let mut response = client
		.post(format!("{}/chat/completions", access.endpoint))
		.bearer_auth(access.bearer.expose_secret())
		.json(&body)
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), 200);
	let chunk = tokio::time::timeout(Duration::from_secs(1), response.chunk())
		.await
		.unwrap()
		.unwrap()
		.unwrap();
	assert!(std::str::from_utf8(&chunk).unwrap().contains("early"));
	assert!(!c.fixture.provider.completed.load(Ordering::SeqCst));
	c.fixture.provider.finish.notify_one();
	assert!(response.text().await.unwrap().contains("prompt_tokens"));
	let claims = c.signer.claims.lock().unwrap();
	assert_eq!(claims.len(), 5);
	assert_eq!(
		claims
			.iter()
			.map(|claim| claim.jti)
			.collect::<std::collections::HashSet<_>>()
			.len(),
		5
	);
	assert!(claims.iter().all(|claim| matches!(&claim.sub,TokenSubject::Run{run,call} if run==&c.context.run.unwrap().to_string() && !call.is_nil())));
	assert!(
		claims
			.iter()
			.any(|claim| claim.ops == vec![Operation::Discovery])
	);
	assert!(
		claims
			.iter()
			.any(|claim| claim.ops == vec![Operation::Embeddings])
	);
	assert_eq!(c.fixture.provider.calls.load(Ordering::SeqCst), 6);
	assert!(
		c.fixture
			.logs
			.0
			.lock()
			.unwrap()
			.iter()
			.all(|line| !line.contains(CANARY))
	);
}

#[tokio::test]
async fn mint_rechecks_metadata_and_run_pin_but_maintenance_uses_current_local_binding() {
	let c = Cloud::new(false).await;
	let source = AccessSource::Tenant {
		provider: "openrouter".into(),
	};
	let context = c.scoped(CallOperation::Chat);
	let id = context.provider_credential_id.unwrap();
	let original = c.repo.rows.lock().unwrap()[&id].clone();
	for change in 0..9 {
		let mut record = original.clone();
		match change {
			0 => record.tenant = "foreign".into(),
			1 => record.state = CredentialState::Pending,
			2 => record.state = CredentialState::Revoked,
			3 => record.state = CredentialState::Deleted,
			4 => record.pinned_version = None,
			5 => record.base_url = "http://attacker.invalid".into(),
			6 => {
				record.pinned_version = Some(format!("{}/versions/latest", record.secret_resource))
			}
			7 => record.secret_resource = "projects/other/secrets/foreign".into(),
			8 => record.pinned_version = Some(format!("{}/versions/01", record.secret_resource)),
			_ => unreachable!(),
		}
		c.repo.rows.lock().unwrap().insert(id, record);
		let error = c
			.access
			.resolve(&context, Provider::Openrouter.base_url(), &source)
			.await
			.unwrap_err();
		assert!(!error.to_string().contains(CANARY));
	}
	assert_eq!(c.signer.claims.lock().unwrap().len(), 0);
	assert_eq!(c.fixture.source.reads.load(Ordering::SeqCst), 0);
	c.repo.rows.lock().unwrap().insert(id, original);
	let other = Uuid::now_v7();
	c.repo
		.rows
		.lock()
		.unwrap()
		.insert(other, row(other, "tenant-a", "3"));
	*c.repo.bindings.lock().unwrap() = vec![Binding {
		tenant: "tenant-a".into(),
		provider: Provider::Openrouter,
		provider_credential_id: Some(other),
		revision: 1,
	}];
	c.access
		.resolve(&context, Provider::Openrouter.base_url(), &source)
		.await
		.unwrap();
	assert_eq!(
		c.signer.claims.lock().unwrap().last().unwrap().credential,
		id
	);
	c.repo
		.rows
		.lock()
		.unwrap()
		.insert(id, row(id, "tenant-a", "2"));
	c.access
		.resolve(&context, Provider::Openrouter.base_url(), &source)
		.await
		.unwrap();
	assert_eq!(c.signer.claims.lock().unwrap().last().unwrap().version, "2");
	for purpose in [
		MaintenancePurpose::MemoryIndexing,
		MaintenancePurpose::MemoryRetention,
		MaintenancePurpose::MemoryReflection,
		MaintenancePurpose::MemoryRetrieval,
	] {
		let mut maintenance = context.clone();
		maintenance.run = None;
		maintenance.maintenance = Some(purpose);
		c.access
			.resolve(&maintenance, Provider::Openrouter.base_url(), &source)
			.await
			.unwrap();
		assert_eq!(
			c.signer.claims.lock().unwrap().last().unwrap().credential,
			other
		);
		assert!(
			matches!(&c.signer.claims.lock().unwrap().last().unwrap().sub,TokenSubject::Maintenance{tenant,..} if tenant=="tenant-a")
		);
	}
	// A receiving node passes mapped local authority; remote metadata cannot select a key.
	let local = Uuid::now_v7();
	c.repo
		.rows
		.lock()
		.unwrap()
		.insert(local, row(local, "mapped-local", "1"));
	*c.repo.bindings.lock().unwrap() = vec![Binding {
		tenant: "mapped-local".into(),
		provider: Provider::Openrouter,
		provider_credential_id: Some(local),
		revision: 1,
	}];
	let mut mapped = context;
	mapped.run = None;
	mapped.maintenance = Some(MaintenancePurpose::MemoryRetrieval);
	mapped.tenant = "mapped-local".into();
	c.access
		.resolve(&mapped, Provider::Openrouter.base_url(), &source)
		.await
		.unwrap();
	assert_eq!(
		c.signer.claims.lock().unwrap().last().unwrap().tenant,
		"mapped-local"
	);
	assert_eq!(
		c.signer.claims.lock().unwrap().last().unwrap().credential,
		local
	);
}

#[tokio::test]
async fn expired_capability_is_non_retryable_without_remint_or_key_read() {
	let c = Cloud::new(true).await;
	let provider = aidash_integrations::inference::provider(
		reqwest::Client::new(),
		c.config(),
		c.access.clone(),
		c.context.clone(),
	)
	.unwrap();
	let mut request = Cloud::request();
	request.instructions = CANARY.into();
	let error = provider.infer(request).await.unwrap_err();
	assert!(matches!(error, aidash_application::Error::Invalid(_)));
	assert!(error.to_string().contains("capability_expired"));
	assert!(!error.to_string().contains(CANARY));
	assert_eq!(c.signer.claims.lock().unwrap().len(), 1);
	assert_eq!(c.broker_calls.load(Ordering::SeqCst), 1);
	assert_eq!(c.fixture.source.reads.load(Ordering::SeqCst), 0);
	assert_eq!(c.fixture.provider.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn runless_memory_inference_mints_exact_purposes_and_rebinds_per_call() {
	let c = Cloud::new(false).await;
	let id = c.context.provider_credential_id.unwrap();
	*c.repo.bindings.lock().unwrap() = vec![Binding {
		tenant: "tenant-a".into(),
		provider: Provider::Openrouter,
		provider_credential_id: Some(id),
		revision: 1,
	}];
	for (purpose, expected) in [
		(MaintenancePurpose::MemoryIndexing, "memory_indexing"),
		(MaintenancePurpose::MemoryRetention, "memory_retention"),
		(MaintenancePurpose::MemoryReflection, "memory_reflection"),
		(MaintenancePurpose::MemoryRetrieval, "memory_retrieval"),
	] {
		let mut context = c.context.clone();
		context.run = None;
		context.maintenance = Some(purpose);
		let provider = aidash_integrations::inference::provider(
			reqwest::Client::new(),
			c.config(),
			c.access.clone(),
			context,
		)
		.unwrap();
		assert_eq!(provider.infer(Cloud::request()).await.unwrap().text, "ok");
		let claim = c.signer.claims.lock().unwrap().last().unwrap().clone();
		assert_eq!(
			serde_json::to_value(claim.sub).unwrap(),
			json!({"maintenance":expected,"tenant":"tenant-a"})
		);
		assert_eq!(claim.credential, id);
	}
	let replacement = Uuid::now_v7();
	c.repo
		.rows
		.lock()
		.unwrap()
		.insert(replacement, row(replacement, "tenant-a", "2"));
	c.repo.bindings.lock().unwrap()[0].provider_credential_id = Some(replacement);
	let mut context = c.context.clone();
	context.run = None;
	context.maintenance = Some(MaintenancePurpose::MemoryIndexing);
	let config = aidash_domain::semantic::EmbeddingConfig {
		provider: "openrouter".into(),
		endpoint: Provider::Openrouter.base_url().into(),
		credential_env: None,
		provider_credential: Some("openrouter".into()),
		model: "author/model".into(),
		model_version: "1".into(),
		dimensions: 2,
	};
	aidash_integrations::semantic::embed(
		c.access.as_ref(),
		&context,
		&reqwest::Client::new(),
		&config,
		"test",
	)
	.await
	.unwrap();
	let claim = c.signer.claims.lock().unwrap().last().unwrap().clone();
	assert_eq!(claim.credential, replacement);
	assert_eq!(claim.version, "2");
	assert_eq!(claim.ops, vec![Operation::Embeddings]);
	assert_eq!(c.fixture.provider.calls.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn self_hosted_env_stays_direct_and_resolves_the_key_on_every_call() {
	let c = Cloud::new(false).await;
	let mut config = c.config();
	config.provider_credential = None;
	config.credential_env = Some("AIDASH_SECRET_OPENROUTER".into());
	config.endpoint = c.fixture.broker.catalog["openrouter"].clone();
	config.request_timeout_secs = Some(3601);
	let provider = aidash_integrations::inference::provider(
		reqwest::Client::new(),
		config,
		c.access.clone(),
		Context::default(),
	)
	.unwrap();
	assert_eq!(provider.infer(Cloud::request()).await.unwrap().text, "ok");
	*c.env.0.lock().unwrap() = "changed-env-key".into();
	assert_eq!(provider.infer(Cloud::request()).await.unwrap().text, "ok");
	assert_eq!(
		*c.fixture.provider.bearer.lock().unwrap(),
		vec![format!("Bearer {CANARY}"), "Bearer changed-env-key".into()]
	);
	assert_eq!(c.repo.begins.load(Ordering::SeqCst), 0);
	assert_eq!(c.signer.claims.lock().unwrap().len(), 0);
	assert_eq!(c.fixture.source.reads.load(Ordering::SeqCst), 0);
}
