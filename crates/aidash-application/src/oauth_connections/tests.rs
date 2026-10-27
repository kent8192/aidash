use super::*;
use std::{
	collections::BTreeMap,
	sync::{
		Mutex as StdMutex,
		atomic::{AtomicBool, AtomicUsize, Ordering},
	},
};
use tokio::sync::{Mutex, OwnedMutexGuard};
const TENANT: &str = "brave-otter-k3m9q2xa";
#[derive(Clone, Default)]
struct Data {
	rows: BTreeMap<Uuid, OAuthConnection>,
}
#[derive(Default)]
struct Repo {
	rows: Arc<Mutex<Data>>,
	clients: StdMutex<Vec<DynamicClient>>,
	answers: StdMutex<Vec<(String, String)>>,
	fail_commit: Arc<AtomicBool>,
}
struct Lease {
	guard: OwnedMutexGuard<Data>,
	data: Data,
	owner: Owner,
	fail: Arc<AtomicBool>,
}
#[async_trait]
impl Scope for Lease {
	async fn list(&mut self) -> Result<Vec<OAuthConnection>> {
		Ok(self
			.data
			.rows
			.values()
			.filter(|r| r.tenant == self.owner.tenant && r.subject == self.owner.subject)
			.cloned()
			.collect())
	}
	async fn save(&mut self, r: &OAuthConnection) -> Result<()> {
		self.data.rows.insert(r.id, r.clone());
		Ok(())
	}
	async fn delete(&mut self, id: Uuid) -> Result<()> {
		self.data.rows.remove(&id);
		Ok(())
	}
	async fn cancel_attempts(&mut self, _: Option<&str>) -> Result<()> {
		Ok(())
	}
	async fn commit(mut self: Box<Self>) -> Result<()> {
		if self.fail.swap(false, Ordering::SeqCst) {
			return Err(unavailable());
		}
		*self.guard = self.data.clone();
		Ok(())
	}
}
#[async_trait]
impl Repository for Repo {
	async fn begin(&self, owner: &Owner) -> Result<Box<dyn Scope>> {
		let guard = self.rows.clone().lock_owned().await;
		let data = guard.clone();
		Ok(Box::new(Lease {
			guard,
			data,
			owner: owner.clone(),
			fail: self.fail_commit.clone(),
		}))
	}
	async fn owners(&self, tenant: &str) -> Result<Vec<Owner>> {
		Ok(self
			.rows
			.lock()
			.await
			.rows
			.values()
			.filter(|r| r.tenant == tenant)
			.map(|r| Owner {
				tenant: tenant.into(),
				subject: r.subject.clone(),
			})
			.collect())
	}
	async fn dynamic_client(
		&self,
		tenant: &str,
		issuer: &str,
		redirect_uri: &str,
	) -> Result<Option<DynamicClient>> {
		Ok(self
			.clients
			.lock()
			.unwrap()
			.iter()
			.find(|c| c.tenant == tenant && c.issuer == issuer && c.redirect_uri == redirect_uri)
			.cloned())
	}
	async fn cache_client(&self, c: &DynamicClient) -> Result<DynamicClient> {
		self.clients.lock().unwrap().push(c.clone());
		Ok(c.clone())
	}
	async fn dynamic_clients(&self, tenant: &str) -> Result<Vec<DynamicClient>> {
		Ok(self
			.clients
			.lock()
			.unwrap()
			.iter()
			.filter(|c| c.tenant == tenant)
			.cloned()
			.collect())
	}
	async fn delete_dynamic_client(&self, _: &str, id: Uuid) -> Result<()> {
		self.clients.lock().unwrap().retain(|c| c.id != id);
		Ok(())
	}
	async fn answer_requests(&self, o: &Owner, r: &str) -> Result<()> {
		self.answers
			.lock()
			.unwrap()
			.push((o.subject.clone(), r.into()));
		Ok(())
	}
}
type SecretVersions = BTreeMap<String, String>;
type SecretResources = BTreeMap<String, (String, SecretVersions)>;
#[derive(Default)]
struct Secrets(StdMutex<SecretResources>, AtomicUsize);
/// A pre-Connection Store retains its existing operations and inherits the
/// port's refusal for the new namespace.
struct LegacyStore(Arc<Secrets>);
#[async_trait]
impl Store for LegacyStore {
	fn resource(&self, id: Uuid) -> String {
		self.0.resource(id)
	}
	async fn create(&self, t: &str, id: Uuid) -> Result<()> {
		self.0.create(t, id).await
	}
	async fn add_version(&self, t: &str, r: &str, k: &SecretString) -> Result<String> {
		self.0.add_version(t, r, k).await
	}
	async fn versions(&self, t: &str, r: &str) -> Result<Vec<String>> {
		self.0.versions(t, r).await
	}
	async fn disable(&self, t: &str, v: &str) -> Result<()> {
		self.0.disable(t, v).await
	}
	async fn destroy(&self, t: &str, v: &str) -> Result<()> {
		self.0.destroy(t, v).await
	}
	async fn delete(&self, t: &str, r: &str) -> Result<()> {
		self.0.delete(t, r).await
	}
}
#[async_trait]
impl Store for Secrets {
	fn resource(&self, id: Uuid) -> String {
		self.connection_resource(id)
	}
	async fn create(&self, t: &str, id: Uuid) -> Result<()> {
		self.create_connection(t, id).await
	}
	async fn create_connection(&self, t: &str, id: Uuid) -> Result<()> {
		self.0
			.lock()
			.unwrap()
			.entry(self.connection_resource(id))
			.or_insert_with(|| (t.into(), BTreeMap::new()));
		Ok(())
	}
	async fn add_version(&self, t: &str, r: &str, k: &SecretString) -> Result<String> {
		let mut data = self.0.lock().unwrap();
		let (tenant, values) = data.get_mut(r).ok_or_else(unavailable)?;
		if tenant != t {
			return Err(Error::Forbidden);
		}
		let pin = format!("{r}/versions/{}", self.1.fetch_add(1, Ordering::SeqCst) + 1);
		values.insert(pin.clone(), k.expose_secret().into());
		Ok(pin)
	}
	async fn versions(&self, t: &str, r: &str) -> Result<Vec<String>> {
		let data = self.0.lock().unwrap();
		Ok(data
			.get(r)
			.filter(|(tenant, _)| tenant == t)
			.map(|(_, v)| v.keys().cloned().collect())
			.unwrap_or_default())
	}
	async fn disable(&self, t: &str, v: &str) -> Result<()> {
		self.destroy(t, v).await
	}
	async fn destroy(&self, t: &str, v: &str) -> Result<()> {
		for (_, (tenant, versions)) in self.0.lock().unwrap().iter_mut() {
			if tenant == t {
				versions.remove(v);
			}
		}
		Ok(())
	}
	async fn delete(&self, t: &str, r: &str) -> Result<()> {
		let mut data = self.0.lock().unwrap();
		if data.get(r).is_some_and(|(tenant, _)| tenant != t) {
			return Err(Error::Forbidden);
		}
		data.remove(r);
		Ok(())
	}
}
#[async_trait]
impl KeyMaterialReader for Secrets {
	async fn read(&self, t: &str, r: &str, v: &str) -> Result<SecretString> {
		self.0
			.lock()
			.unwrap()
			.get(r)
			.filter(|(tenant, _)| tenant == t)
			.and_then(|(_, vrs)| vrs.get(v))
			.cloned()
			.map(SecretString::from)
			.ok_or_else(unavailable)
	}
}
#[derive(Default)]
struct OAuth {
	short_refresh: AtomicBool,
	registered: AtomicUsize,
	exchanged: AtomicUsize,
	refreshed: AtomicUsize,
	revoked: AtomicUsize,
	mode: AtomicUsize,
}
fn server() -> AuthorizationServer {
	AuthorizationServer {
		issuer: "https://auth.example".into(),
		authorization_endpoint: Url::parse("https://auth.example/authorize").unwrap(),
		token_endpoint: Url::parse("https://auth.example/token").unwrap(),
		registration_endpoint: Some(Url::parse("https://auth.example/register").unwrap()),
		revocation_endpoint: Some(Url::parse("https://auth.example/revoke").unwrap()),
		scopes_supported: None,
		code_challenge_methods_supported: vec!["S256".into()],
	}
}
#[async_trait]
impl OAuthDiscovery for OAuth {
	async fn protected_resource(&self, r: &str) -> Result<Option<ProtectedResource>> {
		Ok(Some(ProtectedResource {
			resource: r.into(),
			authorization_servers: vec!["https://auth.example".into()],
			scopes_supported: Some(vec!["read".into(), "write".into()]),
		}))
	}
	async fn authorization_server(&self, _: &str) -> Result<AuthorizationServer> {
		Ok(server())
	}
	async fn register(
		&self,
		_: &AuthorizationServer,
		_: &Url,
		_: &str,
	) -> Result<ClientCredentials> {
		self.registered.fetch_add(1, Ordering::SeqCst);
		Ok(ClientCredentials {
			client_id: "dynamic-client".into(),
			client_secret: Some("client-secret".into()),
		})
	}
}
#[async_trait]
impl OAuthTokens for OAuth {
	async fn exchange(
		&self,
		_: &AuthorizationServer,
		c: &ClientCredentials,
		code: &str,
		_: &SecretString,
		_: &Url,
		r: &str,
	) -> Result<Tokens> {
		assert_eq!(c.client_id, "dynamic-client");
		assert_eq!(code, "code");
		assert_eq!(r, "https://mcp.example/mcp");
		self.exchanged.fetch_add(1, Ordering::SeqCst);
		Ok(Tokens {
			access_token: "access-1".into(),
			refresh_token: Some("refresh-1".into()),
			expires_in: Some(match self.mode.load(Ordering::SeqCst) {
				5 => u64::try_from(i64::MAX).unwrap(),
				6 => u64::MAX,
				_ => 1,
			}),
			scope: (self.mode.load(Ordering::SeqCst) == 4).then(|| "write".into()),
		})
	}
	async fn refresh(
		&self,
		_: &AuthorizationServer,
		_: &ClientCredentials,
		token: &SecretString,
		_: &str,
	) -> std::result::Result<Tokens, RefreshFailure> {
		assert_eq!(token.expose_secret(), "refresh-1");
		self.refreshed.fetch_add(1, Ordering::SeqCst);
		match self.mode.load(Ordering::SeqCst) {
			1 => Err(RefreshFailure::Ambiguous(unavailable())),
			2 => Err(RefreshFailure::Unavailable(unavailable())),
			3 => Err(RefreshFailure::Terminal),
			_ => Ok(Tokens {
				access_token: "access-2".into(),
				refresh_token: None,
				expires_in: Some(if self.short_refresh.load(Ordering::SeqCst) {
					1
				} else {
					3600
				}),
				scope: None,
			}),
		}
	}
	async fn revoke(
		&self,
		_: &AuthorizationServer,
		_: &ClientCredentials,
		_: &SecretString,
	) -> Result<()> {
		self.revoked.fetch_add(1, Ordering::SeqCst);
		Ok(())
	}
}
struct Policy;
#[async_trait]
impl ConnectionPolicy for Policy {
	async fn registered_clients(
		&self,
		_: &str,
		_: &str,
		_: &str,
	) -> Result<(Option<domain::OAuthClient>, Option<domain::OAuthClient>)> {
		Ok((None, None))
	}
	async fn scope_ceiling(&self, _: &str, _: &str) -> Result<Option<Vec<String>>> {
		Ok(Some(vec!["read".into()]))
	}
}
struct Lookup;
#[async_trait]
impl DefinitionLookup for Lookup {
	async fn definition(&mut self, _: &str, _: &str) -> Result<aidash_domain::registry::Entry> {
		Err(Error::NotFound("tool".into()))
	}
}
struct Fixture {
	service: Service,
	repo: Arc<Repo>,
	secrets: Arc<Secrets>,
	oauth: Arc<OAuth>,
	owner: Owner,
}
impl Fixture {
	fn new() -> Self {
		let repo = Arc::new(Repo::default());
		let secrets = Arc::new(Secrets::default());
		let oauth = Arc::new(OAuth::default());
		Self {
			service: Service {
				repository: repo.clone(),
				store: secrets.clone(),
				reader: secrets.clone(),
				discovery: oauth.clone(),
				tokens: oauth.clone(),
				policy: Arc::new(Policy),
			},
			repo,
			secrets,
			oauth,
			owner: Owner {
				tenant: TENANT.into(),
				subject: "alice".into(),
			},
		}
	}
	async fn begin(&self) -> Begin {
		self.service
			.begin(
				&self.owner,
				"https://mcp.example/mcp/",
				&["read".into()],
				None,
				&mut Lookup,
				&Url::parse("https://aidash.example/auth/connections/callback").unwrap(),
				"/settings",
			)
			.await
			.unwrap()
	}
	async fn connect(&self) -> Uuid {
		let b = self.begin().await;
		let id = b.pending.connection;
		assert_eq!(
			self.service
				.complete(b.pending, Some("code"), None)
				.await
				.unwrap(),
			Outcome::Connected
		);
		id
	}
}
#[tokio::test]
async fn oauth_connections_begin_refuses_a_store_without_connection_support() {
	let mut f = Fixture::new();
	f.service.store = Arc::new(LegacyStore(f.secrets.clone()));
	let result = f
		.service
		.begin(
			&f.owner,
			"https://mcp.example/mcp",
			&["read".into()],
			None,
			&mut Lookup,
			&Url::parse("https://aidash.example/auth/connections/callback").unwrap(),
			"/settings",
		)
		.await;
	assert!(matches!(result, Err(Error::ConnectionStoreUnavailable)));
	assert!(f.repo.rows.lock().await.rows.is_empty());
	assert!(f.secrets.0.lock().unwrap().is_empty());
	assert_eq!(f.oauth.exchanged.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn oauth_connections_owner_refresh_and_disconnect_lifecycle() {
	let f = Fixture::new();
	let id = f.connect().await;
	let other = Owner {
		tenant: TENANT.into(),
		subject: "bob".into(),
	};
	assert!(
		matches!(f.service.bearer(&other,"https://mcp.example/mcp").await,Err(Error::Invalid(s)) if s==domain::CONNECTION_REQUIRED)
	);
	let bearer = f
		.service
		.bearer(&f.owner, "https://mcp.example/mcp")
		.await
		.unwrap();
	assert_eq!(bearer.access_token.expose_secret(), "access-2");
	assert_eq!(bearer.connection, id);
	let row = f.repo.rows.lock().await.rows[&id].clone();
	let secret = f.service.read(&row, &row.pinned_version).await.unwrap();
	let Journal::Tokens(tokens) = secret else {
		panic!()
	};
	assert_eq!(tokens.refresh_token.as_deref(), Some("refresh-1"));
	assert_eq!(tokens.scope, ["read"]);
	assert_eq!(
		*f.repo.answers.lock().unwrap(),
		[("alice".into(), "https://mcp.example/mcp".into())]
	);
	assert!(f.service.disconnect(&other, id).await.is_err());
	assert!(f.service.disconnect(&f.owner, id).await.unwrap().revoked);
	assert!(f.service.list(&f.owner).await.unwrap().is_empty());
	assert!(
		f.secrets
			.versions(TENANT, &row.secret_resource)
			.await
			.unwrap()
			.is_empty()
	);
	assert!(f.oauth.revoked.load(Ordering::SeqCst) > 0);
}
#[tokio::test]
async fn oauth_connections_recovers_receipt_after_failed_database_commit_without_refresh_replay() {
	let f = Fixture::new();
	f.connect().await;
	f.repo.fail_commit.store(true, Ordering::SeqCst);
	assert!(
		f.service
			.bearer(&f.owner, "https://mcp.example/mcp")
			.await
			.is_err()
	);
	let recovered = f
		.service
		.bearer(&f.owner, "https://mcp.example/mcp")
		.await
		.unwrap();
	assert_eq!(recovered.access_token.expose_secret(), "access-2");
	assert_eq!(f.oauth.refreshed.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn oauth_connections_ambiguous_rotation_requires_reconnect_and_never_replays_token() {
	let f = Fixture::new();
	f.connect().await;
	f.oauth.mode.store(1, Ordering::SeqCst);
	assert!(
		matches!(f.service.bearer(&f.owner,"https://mcp.example/mcp").await,Err(Error::External(s)) if s==domain::CONNECTION_UNAVAILABLE)
	);
	assert!(
		matches!(f.service.bearer(&f.owner,"https://mcp.example/mcp").await,Err(Error::Invalid(s)) if s==domain::CONNECTION_REQUIRED)
	);
	assert_eq!(f.oauth.refreshed.load(Ordering::SeqCst), 1);
	assert_eq!(
		f.service.list(&f.owner).await.unwrap()[0].state,
		ConnectionState::NeedsReconnection
	);
}
#[tokio::test]
async fn oauth_connections_unavailable_refresh_keeps_grant_for_retry() {
	let f = Fixture::new();
	f.connect().await;
	f.oauth.mode.store(2, Ordering::SeqCst);
	assert!(
		f.service
			.bearer(&f.owner, "https://mcp.example/mcp")
			.await
			.is_err()
	);
	f.oauth.mode.store(0, Ordering::SeqCst);
	assert!(
		f.service
			.bearer(&f.owner, "https://mcp.example/mcp")
			.await
			.is_ok()
	);
	assert_eq!(f.oauth.refreshed.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn oauth_connections_scope_ceiling_cache_pkce_and_stale_callback() {
	let f = Fixture::new();
	let b = f.begin().await;
	let url = Url::parse(&b.authorization_url).unwrap();
	let pairs: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
	assert_eq!(pairs["resource"], "https://mcp.example/mcp");
	assert_eq!(pairs["scope"], "read");
	assert_eq!(pairs["code_challenge_method"], "S256");
	assert_ne!(pairs["code_challenge"], b.pending.pkce_verifier);
	let b2 = f.begin().await;
	assert_eq!(f.oauth.registered.load(Ordering::SeqCst), 1);
	assert!(
		f.service
			.begin(
				&f.owner,
				"https://mcp.example/mcp",
				&["write".into()],
				None,
				&mut Lookup,
				&Url::parse("https://aidash.example/auth/connections/callback").unwrap(),
				"/settings"
			)
			.await
			.is_err()
	);
	f.service
		.disconnect(&f.owner, b2.pending.connection)
		.await
		.unwrap();
	assert!(
		f.service
			.complete(b.pending, Some("code"), None)
			.await
			.is_err()
	);
	assert_eq!(f.oauth.exchanged.load(Ordering::SeqCst), 0);
	f.service.retire_tenant(TENANT).await.unwrap();
	assert!(f.secrets.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn oauth_connections_recovered_rotation_commits_before_a_second_external_refresh() {
	let f = Fixture::new();
	f.connect().await;
	f.oauth.short_refresh.store(true, Ordering::SeqCst);
	f.repo.fail_commit.store(true, Ordering::SeqCst);
	assert!(
		f.service
			.bearer(&f.owner, "https://mcp.example/mcp")
			.await
			.is_err()
	);
	f.repo.fail_commit.store(true, Ordering::SeqCst);
	assert!(
		f.service
			.bearer(&f.owner, "https://mcp.example/mcp")
			.await
			.is_err()
	);
	assert_eq!(
		f.oauth.refreshed.load(Ordering::SeqCst),
		1,
		"a recovered grant needs a durable base before another rotating effect"
	);
}

struct ToolLookup {
	endpoint: &'static str,
	scopes: Vec<String>,
}
#[async_trait]
impl DefinitionLookup for ToolLookup {
	async fn definition(
		&mut self,
		id: &str,
		version: &str,
	) -> Result<aidash_domain::registry::Entry> {
		Ok(serde_json::from_value(
			serde_json::json!({"id":id,"version":version,"kind":"tool","name":{"en":"MCP tool"},"description":{"en":"MCP tool"},"config":{"transport":"mcp","endpoint":self.endpoint,"tool_name":"read","replay":"read_only","idempotency_argument":null,"scopes":self.scopes}}),
		)?)
	}
}
#[tokio::test]
async fn oauth_connections_tool_reference_bounds_scopes_and_the_exact_resource() {
	for (endpoint, scopes, allowed) in [
		("https://mcp.example/mcp", vec!["read".into()], true),
		("https://mcp.example/mcp", vec![], false),
		("https://other.example/mcp", vec!["read".into()], false),
	] {
		let f = Fixture::new();
		let result = f
			.service
			.begin(
				&f.owner,
				"https://mcp.example/mcp",
				&["read".into()],
				Some(&ToolRef {
					id: "tool".into(),
					version: "1.0.0".into(),
				}),
				&mut ToolLookup { endpoint, scopes },
				&Url::parse("https://aidash.example/auth/connections/callback").unwrap(),
				"/settings",
			)
			.await;
		assert_eq!(result.is_ok(), allowed);
		assert_eq!(
			f.oauth.registered.load(Ordering::SeqCst),
			usize::from(allowed)
		);
	}
}
#[tokio::test]
async fn oauth_connections_rejects_and_revokes_unusable_grants() {
	for mode in [4, 5, 6] {
		let f = Fixture::new();
		let b = f.begin().await;
		f.oauth.mode.store(mode, Ordering::SeqCst);
		assert!(
			f.service
				.complete(b.pending, Some("code"), None)
				.await
				.is_err()
		);
		assert_eq!(f.oauth.revoked.load(Ordering::SeqCst), 1);
		assert_eq!(
			f.service.list(&f.owner).await.unwrap()[0].state,
			ConnectionState::Pending
		);
	}
}

#[tokio::test]
async fn oauth_connections_delayed_cleanup_preserves_a_later_refresh_intent() {
	let f = Fixture::new();
	let id = f.connect().await;
	let row = f.repo.rows.lock().await.rows[&id].clone();
	let intent = Intent {
		connection: id,
		revision: row.revision,
		pin: row.pinned_version.clone(),
	};
	f.secrets
		.add_version(
			TENANT,
			&row.secret_resource,
			&seal(&Journal::Intent {
				refresh_intent: intent,
			})
			.unwrap(),
		)
		.await
		.unwrap();
	f.service.prune(&row).await.unwrap();
	assert!(
		matches!(f.service.bearer(&f.owner,"https://mcp.example/mcp").await,Err(Error::Invalid(code)) if code==domain::CONNECTION_REQUIRED)
	);
	assert_eq!(f.oauth.refreshed.load(Ordering::SeqCst), 0);
}
