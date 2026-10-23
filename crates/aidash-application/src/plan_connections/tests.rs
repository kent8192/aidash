use super::*;
use std::collections::BTreeMap;
use std::sync::Mutex as StdMutex;
use tokio::sync::{Mutex, OwnedMutexGuard};

const TENANT: &str = "brave-otter-k3m9q2xa";
const HOST: &str = "urn:uuid:0d5c0e5e-5d5a-4c4b-9a8e-1b2c3d4e5f60";
/// The External Identity whose session connects in these tests.
const IDENTITY: Uuid = Uuid::from_u128(1);
/// Another External Identity mapped to the same subject.
const OTHER: Uuid = Uuid::from_u128(2);

/// `n` active identities mapped to the owner, [`IDENTITY`] first.
fn mapped(n: u128) -> Vec<MappedIdentity> {
	(1..=n)
		.map(|id| MappedIdentity {
			id: Uuid::from_u128(id),
			disabled: false,
		})
		.collect()
}

#[derive(Clone, Default)]
struct Data {
	rows: BTreeMap<Uuid, PlanConnection>,
	mapped: Vec<MappedIdentity>,
	events: Vec<String>,
	/// Pending attempts ended by a committed disconnect.
	cancelled_attempts: usize,
	/// Commits that fail before applying anything, as a lost database would.
	failing_commits: usize,
	/// Commits that succeed before `failing_commits` take effect.
	passing_commits: usize,
}
#[derive(Clone)]
struct Repo(Arc<Mutex<Data>>);
impl Repo {
	fn new(identities: u128) -> Self {
		Self(Arc::new(Mutex::new(Data {
			mapped: mapped(identities),
			..Data::default()
		})))
	}
}
struct Lease {
	guard: OwnedMutexGuard<Data>,
	data: Data,
	owner: Owner,
}
#[async_trait]
impl Repository for Repo {
	async fn begin(&self, owner: &Owner) -> Result<Box<dyn Scope>> {
		let guard = self.0.clone().lock_owned().await;
		let data = guard.clone();
		Ok(Box::new(Lease {
			guard,
			data,
			owner: owner.clone(),
		}))
	}
	async fn owner_of(&self, id: Uuid) -> Result<Option<Owner>> {
		Ok(self.0.lock().await.rows.get(&id).map(|row| Owner {
			tenant: row.tenant.clone(),
			subject: row.subject.clone(),
		}))
	}
	async fn owners(&self, tenant: &str) -> Result<Vec<Owner>> {
		Ok(self
			.0
			.lock()
			.await
			.rows
			.values()
			.filter(|row| row.tenant == tenant)
			.map(|row| Owner {
				tenant: row.tenant.clone(),
				subject: row.subject.clone(),
			})
			.collect())
	}
}
#[async_trait]
impl Scope for Lease {
	async fn owned(&mut self, provider: Provider) -> Result<Option<PlanConnection>> {
		Ok(self
			.data
			.rows
			.values()
			.find(|row| {
				row.tenant == self.owner.tenant
					&& row.subject == self.owner.subject
					&& row.provider == provider
			})
			.cloned())
	}
	async fn mapped_identities(&mut self) -> Result<Vec<MappedIdentity>> {
		Ok(self.data.mapped.clone())
	}
	async fn insert(&mut self, value: &PlanConnection, event: &str, _: &str) -> Result<()> {
		assert!(self.data.rows.insert(value.id, value.clone()).is_none());
		self.data.events.push(event.into());
		Ok(())
	}
	async fn save(&mut self, value: &PlanConnection, event: &str, _: &str) -> Result<()> {
		self.data.rows.insert(value.id, value.clone());
		self.data.events.push(event.into());
		Ok(())
	}
	async fn delete(&mut self, value: &PlanConnection, event: &str, _: &str) -> Result<()> {
		self.data.rows.remove(&value.id);
		self.data.events.push(event.into());
		Ok(())
	}
	async fn cancel_attempts(&mut self, _: Provider) -> Result<()> {
		self.data.cancelled_attempts += 1;
		Ok(())
	}
	async fn commit(mut self: Box<Self>) -> Result<()> {
		if self.guard.passing_commits > 0 {
			self.guard.passing_commits -= 1;
		} else if self.guard.failing_commits > 0 {
			self.guard.failing_commits -= 1;
			return Err(Error::External("database unavailable".into()));
		}
		let failing = self.guard.failing_commits;
		let passing = self.guard.passing_commits;
		*self.guard = Data {
			failing_commits: failing,
			passing_commits: passing,
			..self.data.clone()
		};
		Ok(())
	}
}

/// Plan Token versions by resource, and Store operations set to fail.
#[derive(Clone, Default)]
struct FakeStore(
	Arc<StdMutex<BTreeMap<String, BTreeMap<u32, String>>>>,
	Arc<StdMutex<Failing>>,
);
#[derive(Default)]
struct Failing {
	add_version: bool,
	passing_versions: usize,
	destroy: bool,
	delete: bool,
}
impl FakeStore {
	fn plaintexts(&self) -> Vec<String> {
		self.0
			.lock()
			.unwrap()
			.values()
			.flat_map(|versions| versions.values().cloned())
			.collect()
	}
	fn resources(&self) -> usize {
		self.0.lock().unwrap().len()
	}
}
fn split(version: &str) -> (&str, u32) {
	let (resource, n) = version.rsplit_once("/versions/").unwrap();
	(resource, n.parse().unwrap())
}
#[async_trait]
impl Store for FakeStore {
	fn resource(&self, id: Uuid) -> String {
		format!("fake/{id}")
	}
	async fn create(&self, _: &str, _: Uuid) -> Result<()> {
		unreachable!("Plan Connections never create Provider Credential resources")
	}
	async fn create_plan(&self, _: &str, id: Uuid) -> Result<()> {
		self.0
			.lock()
			.unwrap()
			.insert(self.plan_resource(id), BTreeMap::new());
		Ok(())
	}
	async fn add_version(&self, _: &str, resource: &str, key: &SecretString) -> Result<String> {
		{
			let mut faults = self.1.lock().unwrap();
			if faults.passing_versions > 0 {
				faults.passing_versions -= 1;
			} else if faults.add_version {
				return Err(Error::External("store unavailable".into()));
			}
		}
		let mut state = self.0.lock().unwrap();
		let versions = state.get_mut(resource).unwrap();
		let n = versions.keys().max().copied().unwrap_or(0) + 1;
		versions.insert(n, key.expose_secret().to_owned());
		Ok(format!("{resource}/versions/{n}"))
	}
	async fn versions(&self, _tenant: &str, resource: &str) -> Result<Vec<String>> {
		Ok(self
			.0
			.lock()
			.unwrap()
			.get(resource)
			.map(|versions| {
				versions
					.keys()
					.map(|n| format!("{resource}/versions/{n}"))
					.collect()
			})
			.unwrap_or_default())
	}
	async fn disable(&self, _tenant: &str, _: &str) -> Result<()> {
		Ok(())
	}
	async fn destroy(&self, _tenant: &str, version: &str) -> Result<()> {
		if self.1.lock().unwrap().destroy {
			return Err(Error::External("store unavailable".into()));
		}
		let (resource, n) = split(version);
		if let Some(versions) = self.0.lock().unwrap().get_mut(resource) {
			versions.remove(&n);
		}
		Ok(())
	}
	async fn delete(&self, _tenant: &str, resource: &str) -> Result<()> {
		if self.1.lock().unwrap().delete {
			return Err(Error::External("store unavailable".into()));
		}
		self.0.lock().unwrap().remove(resource);
		Ok(())
	}
}
#[async_trait]
impl KeyMaterialReader for FakeStore {
	async fn read(&self, _: &str, resource: &str, version: &str) -> Result<SecretString> {
		let (pinned, n) = split(version);
		assert_eq!(pinned, resource);
		self.0
			.lock()
			.unwrap()
			.get(resource)
			.and_then(|versions| versions.get(&n))
			.map(|value| SecretString::from(value.clone()))
			.ok_or_else(|| Error::Invalid("missing version".into()))
	}
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RefreshMode {
	Ok,
	Terminal,
	Unavailable,
	Ambiguous,
	WithoutPlanScope,
}
struct Calls {
	exchanges: usize,
	refreshes: usize,
	revoked: Vec<String>,
}
struct FakeAuthorization {
	scope: Option<String>,
	id_token_valid: bool,
	/// The verified ChatGPT account of each exchange.
	account: StdMutex<String>,
	/// `/models` rejects this many access tokens like an early expiry.
	models_reject: StdMutex<usize>,
	/// `/models` reports that the owner's plan is not eligible.
	models_ineligible: bool,
	/// A reconnect that completes while the next revocation is in flight.
	reconnect_during_revoke: StdMutex<Option<Repo>>,
	refresh: RefreshMode,
	revoke_fails: bool,
	calls: StdMutex<Calls>,
}
impl Default for FakeAuthorization {
	fn default() -> Self {
		Self {
			scope: Some(aidash_domain::plan_connections::SCOPES.into()),
			id_token_valid: true,
			account: StdMutex::new("user-1".into()),
			models_reject: StdMutex::new(0),
			models_ineligible: false,
			reconnect_during_revoke: StdMutex::new(None),
			refresh: RefreshMode::Ok,
			revoke_fails: false,
			calls: StdMutex::new(Calls {
				exchanges: 0,
				refreshes: 0,
				revoked: Vec::new(),
			}),
		}
	}
}
fn issued(access: &str, refresh: &str, scope: Option<&str>) -> Issued {
	Issued {
		access_token: access.to_owned().into(),
		refresh_token: refresh.to_owned().into(),
		id_token: Some("id-token-canary".to_owned().into()),
		expires_in: 3600,
		scope: scope.map(str::to_owned),
		earliest_refresh_at: None,
	}
}
#[async_trait]
impl Authorization for FakeAuthorization {
	async fn exchange(&self, request: ExchangeRequest<'_>) -> Result<Issued> {
		assert_eq!(request.code, "code-1");
		assert_eq!(request.redirect_uri, "http://127.0.0.1:43123/auth/callback");
		assert_ne!(
			request.client_id,
			aidash_domain::plan_connections::DYNAMIC_CLIENT
		);
		self.calls.lock().unwrap().exchanges += 1;
		Ok(issued("access-1", "refresh-1", self.scope.as_deref()))
	}
	async fn verify_id_token(
		&self,
		_: &SecretString,
		client_id: &str,
		nonce: &str,
	) -> Result<String> {
		assert_eq!(nonce, "nonce-1");
		assert!(client_id.starts_with("oaiapp_"));
		if self.id_token_valid {
			Ok(self.account.lock().unwrap().clone())
		} else {
			Err(Error::Unauthorized)
		}
	}
	async fn refresh(
		&self,
		_: &str,
		refresh_token: &SecretString,
	) -> std::result::Result<Issued, RefreshFailure> {
		let n = {
			let mut calls = self.calls.lock().unwrap();
			calls.refreshes += 1;
			calls.refreshes
		};
		assert!(refresh_token.expose_secret().starts_with("refresh-"));
		match self.refresh {
			RefreshMode::Ok => Ok(issued(
				&format!("access-r{n}"),
				&format!("refresh-r{n}"),
				None,
			)),
			RefreshMode::WithoutPlanScope => Ok(issued(
				"access-x",
				"refresh-x",
				Some("openid offline_access"),
			)),
			RefreshMode::Terminal => Err(RefreshFailure::Terminal),
			RefreshMode::Ambiguous => Err(RefreshFailure::Ambiguous(Error::External(
				"lost refresh response".into(),
			))),
			RefreshMode::Unavailable => Err(RefreshFailure::Unavailable(Error::ProviderRejected {
				status: 503,
				reason: "unavailable".into(),
			})),
		}
	}
	async fn revoke(&self, _: &str, refresh_token: &SecretString) -> Result<()> {
		let reconnect = self.reconnect_during_revoke.lock().unwrap().take();
		if let Some(repo) = reconnect {
			let mut data = repo.0.lock().await;
			let row = data.rows.values_mut().next().unwrap();
			row.revision += 1;
			row.pinned_version = Some(format!("{}/versions/99", row.secret_resource));
		}
		self.calls
			.lock()
			.unwrap()
			.revoked
			.push(refresh_token.expose_secret().to_owned());
		if self.revoke_fails {
			return Err(Error::External("revocation unavailable".into()));
		}
		Ok(())
	}
	async fn models(&self, access_token: &SecretString) -> Result<Vec<PlanModel>> {
		assert!(access_token.expose_secret().starts_with("access-"));
		if self.models_ineligible {
			return Err(Error::Plan(Failure::Ineligible));
		}
		{
			let mut rejections = self.models_reject.lock().unwrap();
			if *rejections > 0 {
				*rejections -= 1;
				return Err(Error::Plan(Failure::NeedsReconnection));
			}
		}
		Ok(vec![PlanModel {
			slug: "gpt-fixture".into(),
			display_name: "GPT Fixture".into(),
			context_window: 128_000,
			max_output_tokens: 16_384,
			defaulted: true,
		}])
	}
}

struct Fixture {
	repo: Repo,
	store: FakeStore,
	authorization: Arc<FakeAuthorization>,
	service: Service,
}
fn fixture(identities: u128, authorization: FakeAuthorization) -> Fixture {
	let repo = Repo::new(identities);
	let store = FakeStore::default();
	let authorization = Arc::new(authorization);
	let service = Service {
		repository: Arc::new(repo.clone()),
		store: Arc::new(store.clone()),
		reader: Arc::new(store.clone()),
		authorization: authorization.clone(),
	};
	Fixture {
		repo,
		store,
		authorization,
		service,
	}
}
fn owner() -> Owner {
	Owner {
		tenant: TENANT.into(),
		subject: "alice".into(),
	}
}
fn callback(client_id: Option<&str>) -> Callback {
	Callback {
		state: "state-1".into(),
		result: CallbackResult::Code {
			code: "code-1".into(),
			client_id: client_id.map(str::to_owned),
		},
	}
}
async fn connect(
	fixture: &Fixture,
	saved: Option<&str>,
	returned: Option<&str>,
) -> Result<Outcome> {
	connect_as(fixture, IDENTITY, saved, returned).await
}
async fn connect_as(
	fixture: &Fixture,
	identity: Uuid,
	saved: Option<&str>,
	returned: Option<&str>,
) -> Result<Outcome> {
	let verifier = SecretString::from("verifier-1".to_owned());
	fixture
		.service
		.complete(
			&owner(),
			identity,
			Pending {
				state: "state-1",
				verifier: &verifier,
				nonce: "nonce-1",
				redirect_uri: "http://127.0.0.1:43123/auth/callback",
				saved_client: saved,
			},
			&callback(returned),
			"alice",
		)
		.await
}
async fn only_row(fixture: &Fixture) -> PlanConnection {
	let data = fixture.repo.0.lock().await;
	assert_eq!(data.rows.len(), 1);
	data.rows.values().next().unwrap().clone()
}
async fn set_row(fixture: &Fixture, change: impl FnOnce(&mut PlanConnection)) {
	let mut data = fixture.repo.0.lock().await;
	change(data.rows.values_mut().next().unwrap());
}

#[test]
fn first_authorizations_register_a_client_for_this_host() {
	let request = begin(HOST, 43123, None).unwrap();
	let url = url::Url::parse(&request.url).unwrap();
	assert_eq!(url.host_str(), Some("auth.openai.com"));
	assert_eq!(url.path(), "/api/accounts/authorize");
	let query: BTreeMap<String, String> = url.query_pairs().into_owned().collect();
	assert_eq!(query["response_type"], "code");
	assert_eq!(query["client_id"], "dynamic_agent_client");
	assert_eq!(query["agent_name_hint"], "Aidash");
	assert_eq!(
		query["redirect_uri"],
		"http://127.0.0.1:43123/auth/callback"
	);
	assert_eq!(query["redirect_uri"], request.redirect_uri);
	assert_eq!(
		query["scope"],
		"openid profile email offline_access resource.invoke chatgpt.tokens.use.direct"
	);
	assert_eq!(query["resource"], "https://api.openai.com/v1");
	assert_eq!(query["ext_agent_host_id"], HOST);
	assert_eq!(query["state"], request.state.expose_secret());
	assert_eq!(query["nonce"], request.nonce.expose_secret());
	assert_eq!(query["code_challenge_method"], "S256");
	assert_eq!(query["code_challenge"], challenge(&request.verifier));
	// The verifier stays on the host.
	assert!(!request.url.contains(request.verifier.expose_secret()));
	assert_ne!(request.state.expose_secret(), request.nonce.expose_secret());
}

#[test]
fn reauthorizations_reuse_the_issued_client() {
	let request = begin(
		HOST,
		aidash_domain::plan_connections::PASTE_PORT,
		Some("oaiapp_1"),
	)
	.unwrap();
	let url = url::Url::parse(&request.url).unwrap();
	let query: BTreeMap<String, String> = url.query_pairs().into_owned().collect();
	assert_eq!(query["client_id"], "oaiapp_1");
	assert!(!query.contains_key("agent_name_hint"));
	assert_eq!(query["redirect_uri"], "http://127.0.0.1:1457/auth/callback");
	assert!(begin(HOST, 0, None).is_err());
}

#[tokio::test]
async fn connecting_stores_only_the_access_and_refresh_tokens() {
	let fixture = fixture(1, FakeAuthorization::default());
	assert_eq!(
		connect(&fixture, None, Some("oaiapp_1")).await.unwrap(),
		Outcome::Connected
	);
	let row = only_row(&fixture).await;
	assert_eq!(row.state, State::Active);
	assert_eq!(row.client_id, "oaiapp_1");
	assert_eq!(row.subject, "alice");
	assert!(
		row.secret_resource
			.starts_with("aidash-store/plan-connections/")
	);
	let stored = fixture.store.plaintexts();
	assert_eq!(stored.len(), 1);
	assert!(stored[0].contains("access-1") && stored[0].contains("refresh-1"));
	// The ID token is verified, never kept.
	assert!(!stored[0].contains("id-token-canary"));
	assert!(
		fixture
			.authorization
			.calls
			.lock()
			.unwrap()
			.revoked
			.is_empty()
	);
	let status = fixture.service.status(&owner()).await.unwrap().unwrap();
	assert_eq!(status.state, State::Active);
	// Registered first, then pinned to its tokens.
	assert_eq!(status.revision, 2);
	assert_eq!(
		fixture.repo.0.lock().await.events,
		vec!["plan_connection.registered", "plan_connection.connected"]
	);
}

#[tokio::test]
async fn declined_and_failed_consents_never_reach_the_exchange() {
	let fixture = fixture(1, FakeAuthorization::default());
	let verifier = SecretString::from("verifier-1".to_owned());
	for (result, outcome) in [
		(CallbackResult::Denied, Outcome::Denied),
		(CallbackResult::Failed, Outcome::Failed),
	] {
		let value = fixture
			.service
			.complete(
				&owner(),
				IDENTITY,
				Pending {
					state: "state-1",
					verifier: &verifier,
					nonce: "nonce-1",
					redirect_uri: "http://127.0.0.1:43123/auth/callback",
					saved_client: None,
				},
				&Callback {
					state: "state-1".into(),
					result,
				},
				"alice",
			)
			.await
			.unwrap();
		assert_eq!(value, outcome);
	}
	assert_eq!(fixture.authorization.calls.lock().unwrap().exchanges, 0);
	assert_eq!(fixture.store.resources(), 0);
}

/// OpenAI's recovery guidance keeps a sign-in without plan usage: its tokens
/// are revoked, but the issued client is kept for the next authorization.
#[tokio::test]
async fn grants_without_plan_usage_keep_only_the_issued_client() {
	let fixture = fixture(
		1,
		FakeAuthorization {
			scope: Some("openid profile email offline_access".into()),
			..FakeAuthorization::default()
		},
	);
	assert_eq!(
		connect(&fixture, None, Some("oaiapp_1")).await.unwrap(),
		Outcome::PlanUsageNotGranted
	);
	assert!(fixture.store.plaintexts().is_empty());
	assert_eq!(
		fixture.authorization.calls.lock().unwrap().revoked,
		vec!["refresh-1"]
	);
	let row = only_row(&fixture).await;
	assert_eq!(row.state, State::NeedsReconnection);
	assert!(row.pinned_version.is_none());
	assert_eq!(
		fixture
			.service
			.check(&owner(), IDENTITY)
			.await
			.unwrap()
			.as_deref(),
		Some("oaiapp_1")
	);
	// Asking again reuses the same client and row.
	assert_eq!(
		connect(&fixture, Some("oaiapp_1"), None).await.unwrap(),
		Outcome::PlanUsageNotGranted
	);
	assert_eq!(only_row(&fixture).await.id, row.id);
}

#[tokio::test]
async fn unverified_id_tokens_are_revoked_and_not_stored() {
	let fixture = fixture(
		1,
		FakeAuthorization {
			id_token_valid: false,
			..FakeAuthorization::default()
		},
	);
	assert!(connect(&fixture, None, Some("oaiapp_1")).await.is_err());
	assert_eq!(fixture.store.resources(), 0);
	assert_eq!(
		fixture.authorization.calls.lock().unwrap().revoked,
		vec!["refresh-1"]
	);
}

#[tokio::test]
async fn shared_subjects_cannot_hold_a_plan_connection() {
	let fixture = fixture(2, FakeAuthorization::default());
	assert!(matches!(
		fixture.service.check(&owner(), IDENTITY).await,
		Err(Error::Conflict(_))
	));
	assert!(matches!(
		connect(&fixture, None, Some("oaiapp_1")).await,
		Err(Error::Conflict(_))
	));
	assert_eq!(fixture.store.resources(), 0);
	assert_eq!(
		fixture.authorization.calls.lock().unwrap().revoked,
		vec!["refresh-1"]
	);
}

#[tokio::test]
async fn first_registrations_must_return_an_issued_client() {
	let fixture = fixture(1, FakeAuthorization::default());
	assert!(connect(&fixture, None, None).await.is_err());
	assert_eq!(fixture.authorization.calls.lock().unwrap().exchanges, 0);
}

#[tokio::test]
async fn reconnecting_rotates_tokens_in_place_and_keeps_the_client() {
	let fixture = fixture(1, FakeAuthorization::default());
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	let first = only_row(&fixture).await;
	assert_eq!(
		fixture
			.service
			.check(&owner(), IDENTITY)
			.await
			.unwrap()
			.as_deref(),
		Some("oaiapp_1")
	);
	assert_eq!(
		connect(&fixture, Some("oaiapp_1"), None).await.unwrap(),
		Outcome::Reconnected
	);
	let second = only_row(&fixture).await;
	assert_eq!(second.id, first.id);
	assert_eq!(second.revision, 3);
	assert_ne!(second.pinned_version, first.pinned_version);
	// The replaced version is destroyed; one version remains.
	assert_eq!(fixture.store.plaintexts().len(), 1);
	// A different client returned on reauthorization is refused.
	assert!(
		connect(&fixture, Some("oaiapp_1"), Some("oaiapp_2"))
			.await
			.is_err()
	);
}

#[tokio::test]
async fn fresh_tokens_serve_calls_without_a_refresh() {
	let fixture = fixture(1, FakeAuthorization::default());
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	let row = only_row(&fixture).await;
	let bearer = fixture.service.bearer(row.pin()).await.unwrap();
	assert_eq!(bearer.expose_secret(), "access-1");
	assert_eq!(fixture.authorization.calls.lock().unwrap().refreshes, 0);
}

#[tokio::test]
async fn expiring_tokens_are_refreshed_and_rotated() {
	let fixture = fixture(1, FakeAuthorization::default());
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	set_row(&fixture, |row| row.access_expires_at = Utc::now()).await;
	let row = only_row(&fixture).await;
	let bearer = fixture.service.bearer(row.pin()).await.unwrap();
	assert_eq!(bearer.expose_secret(), "access-r1");
	let refreshed = only_row(&fixture).await;
	assert!(refreshed.refreshed_at.is_some());
	assert!(refreshed.access_expires_at > Utc::now() + Duration::minutes(50));
	let stored = fixture.store.plaintexts();
	assert_eq!(stored.len(), 1);
	assert!(stored[0].contains("refresh-r1"));
	assert!(
		fixture
			.repo
			.0
			.lock()
			.await
			.events
			.contains(&"plan_connection.refreshed".to_owned())
	);
	// The new token serves the next call without another refresh.
	assert_eq!(
		fixture
			.service
			.bearer(row.pin())
			.await
			.unwrap()
			.expose_secret(),
		"access-r1"
	);
	assert_eq!(fixture.authorization.calls.lock().unwrap().refreshes, 1);
}

#[tokio::test]
async fn rejected_refreshes_discard_tokens_and_ask_for_reconnection() {
	let fixture = fixture(
		1,
		FakeAuthorization {
			refresh: RefreshMode::Terminal,
			..FakeAuthorization::default()
		},
	);
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	set_row(&fixture, |row| row.access_expires_at = Utc::now()).await;
	let pin = only_row(&fixture).await.pin();
	assert!(matches!(
		fixture.service.bearer(pin).await,
		Err(Error::Plan(Failure::NeedsReconnection))
	));
	let row = only_row(&fixture).await;
	assert_eq!(row.state, State::NeedsReconnection);
	assert!(row.pinned_version.is_none());
	assert!(fixture.store.plaintexts().is_empty());
	assert!(
		fixture
			.repo
			.0
			.lock()
			.await
			.events
			.contains(&"plan_connection.needs_reconnection".to_owned())
	);
	// Reconnecting makes it usable again.
	assert_eq!(
		connect(&fixture, Some("oaiapp_1"), None).await.unwrap(),
		Outcome::Reconnected
	);
	assert_eq!(only_row(&fixture).await.state, State::Active);
}

#[tokio::test]
async fn refreshes_that_drop_plan_usage_end_the_connection() {
	let fixture = fixture(
		1,
		FakeAuthorization {
			refresh: RefreshMode::WithoutPlanScope,
			..FakeAuthorization::default()
		},
	);
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	set_row(&fixture, |row| row.access_expires_at = Utc::now()).await;
	let pin = only_row(&fixture).await.pin();
	assert!(matches!(
		fixture.service.bearer(pin).await,
		Err(Error::Plan(Failure::NeedsReconnection))
	));
	assert_eq!(
		fixture.authorization.calls.lock().unwrap().revoked,
		vec!["refresh-x"]
	);
}

#[tokio::test]
async fn unavailable_refreshes_keep_the_tokens() {
	let fixture = fixture(
		1,
		FakeAuthorization {
			refresh: RefreshMode::Unavailable,
			..FakeAuthorization::default()
		},
	);
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	set_row(&fixture, |row| row.access_expires_at = Utc::now()).await;
	let before = only_row(&fixture).await;
	assert!(matches!(
		fixture.service.bearer(before.pin()).await,
		Err(Error::ProviderRejected { status: 503, .. })
	));
	let after = only_row(&fixture).await;
	assert_eq!(after.state, State::Active);
	assert_eq!(after.pinned_version, before.pinned_version);
	assert_eq!(fixture.store.plaintexts().len(), 1);
	assert!(matches!(
		fixture.service.bearer(before.pin()).await,
		Err(Error::ProviderRejected { status: 503, .. })
	));
	assert_eq!(fixture.authorization.calls.lock().unwrap().refreshes, 2);
}

#[tokio::test]
async fn unusable_connections_never_serve_calls() {
	let fixture = fixture(1, FakeAuthorization::default());
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	let pin = only_row(&fixture).await.pin();
	set_row(&fixture, |row| {
		row.refresh_expires_at = Utc::now() - Duration::seconds(1)
	})
	.await;
	assert!(matches!(
		fixture.service.bearer(pin).await,
		Err(Error::Plan(Failure::NeedsReconnection))
	));
	// An expired connection records that it needs reconnection.
	let expired = only_row(&fixture).await;
	assert_eq!(expired.state, State::NeedsReconnection);
	assert!(expired.pinned_version.is_none());
	assert!(fixture.store.plaintexts().is_empty());
	assert!(
		fixture
			.repo
			.0
			.lock()
			.await
			.events
			.contains(&"plan_connection.needs_reconnection".to_owned())
	);
	set_row(&fixture, |row| {
		row.refresh_expires_at = Utc::now() + Duration::days(1);
		row.state = State::Ineligible;
	})
	.await;
	assert!(matches!(
		fixture.service.bearer(pin).await,
		Err(Error::Plan(Failure::Ineligible))
	));
	assert!(matches!(
		fixture
			.service
			.bearer(Pin {
				connection: Uuid::now_v7(),
				identity: IDENTITY,
			})
			.await,
		Err(Error::Plan(Failure::NotConnected))
	));
	assert_eq!(fixture.authorization.calls.lock().unwrap().refreshes, 0);
}

#[tokio::test]
async fn a_second_mapping_stops_the_connection_serving_calls() {
	let fixture = fixture(1, FakeAuthorization::default());
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	let pin = only_row(&fixture).await.pin();
	fixture.repo.0.lock().await.mapped = mapped(2);
	assert!(matches!(
		fixture.service.bearer(pin).await,
		Err(Error::Plan(Failure::NotConnected))
	));
}

#[tokio::test]
async fn rejected_access_tokens_force_one_refresh() {
	let fixture = fixture(1, FakeAuthorization::default());
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	let pin = only_row(&fixture).await.pin();
	assert!(fixture.service.refresh_rejected(pin).await.unwrap());
	assert_eq!(fixture.authorization.calls.lock().unwrap().refreshes, 1);
	assert_eq!(
		fixture.service.bearer(pin).await.unwrap().expose_secret(),
		"access-r1"
	);
}

#[tokio::test]
async fn ineligible_plans_mark_the_connection_once() {
	let fixture = fixture(1, FakeAuthorization::default());
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	let pin = only_row(&fixture).await.pin();
	fixture.service.mark_ineligible(pin).await.unwrap();
	fixture.service.mark_ineligible(pin).await.unwrap();
	let row = only_row(&fixture).await;
	assert_eq!(row.state, State::Ineligible);
	assert_eq!(row.revision, 3);
	let events = fixture.repo.0.lock().await.events.clone();
	assert_eq!(
		events
			.iter()
			.filter(|event| *event == "plan_connection.ineligible")
			.count(),
		1
	);
}

/// A reconnection that completes after its connection was disconnected must
/// not bring it back: the owner ended the grant meanwhile.
#[tokio::test]
async fn a_reconnection_completed_after_a_disconnect_is_refused() {
	let fixture = fixture(1, FakeAuthorization::default());
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	// The reconnect started while the connection existed, so it carries the
	// saved client; the owner disconnects before its callback completes.
	let saved = fixture.service.check(&owner(), IDENTITY).await.unwrap();
	assert_eq!(saved.as_deref(), Some("oaiapp_1"));
	fixture
		.service
		.disconnect(&owner(), 2, "alice")
		.await
		.unwrap();
	assert!(matches!(
		connect(&fixture, saved.as_deref(), None).await,
		Err(Error::Conflict(_))
	));
	assert!(fixture.repo.0.lock().await.rows.is_empty());
	assert_eq!(fixture.store.resources(), 0);
	// The tokens the late exchange issued are revoked, not left live.
	assert_eq!(
		fixture.authorization.calls.lock().unwrap().revoked,
		vec!["refresh-1", "refresh-1"]
	);
}

#[tokio::test]
async fn a_disconnect_without_a_connection_still_ends_pending_attempts() {
	let fixture = fixture(1, FakeAuthorization::default());
	assert!(matches!(
		fixture.service.disconnect(&owner(), 1, "alice").await,
		Err(Error::NotFound(_))
	));
	assert_eq!(fixture.repo.0.lock().await.cancelled_attempts, 1);
}

#[tokio::test]
async fn disconnecting_revokes_the_grant_and_deletes_every_copy() {
	let fixture = fixture(1, FakeAuthorization::default());
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	assert!(matches!(
		fixture.service.disconnect(&owner(), 1, "alice").await,
		Err(Error::Domain(aidash_domain::Error::Conflict(_)))
	));
	// A refused disconnect leaves pending attempts to their callbacks.
	assert_eq!(fixture.repo.0.lock().await.cancelled_attempts, 0);
	let result = fixture
		.service
		.disconnect(&owner(), 2, "alice")
		.await
		.unwrap();
	assert!(result.revoked);
	assert_eq!(fixture.repo.0.lock().await.cancelled_attempts, 1);
	assert_eq!(
		fixture.authorization.calls.lock().unwrap().revoked,
		vec!["refresh-1"]
	);
	assert!(fixture.repo.0.lock().await.rows.is_empty());
	assert_eq!(fixture.store.resources(), 0);
	assert!(fixture.service.status(&owner()).await.unwrap().is_none());
}

#[tokio::test]
async fn unconfirmed_revocations_still_delete_aidashs_copy() {
	let fixture = fixture(
		1,
		FakeAuthorization {
			revoke_fails: true,
			..FakeAuthorization::default()
		},
	);
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	let result = fixture
		.service
		.disconnect(&owner(), 2, "alice")
		.await
		.unwrap();
	assert!(!result.revoked);
	assert_eq!(fixture.store.resources(), 0);
}

#[tokio::test]
async fn owners_who_lose_access_lose_their_connection() {
	let fixture = fixture(1, FakeAuthorization::default());
	fixture
		.service
		.retire(&owner(), "deployment_administrator")
		.await
		.unwrap();
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	fixture
		.service
		.retire(&owner(), "deployment_administrator")
		.await
		.unwrap();
	assert!(fixture.repo.0.lock().await.rows.is_empty());
	assert_eq!(fixture.store.resources(), 0);
	assert!(
		fixture
			.repo
			.0
			.lock()
			.await
			.events
			.contains(&"plan_connection.retired".to_owned())
	);
}

#[tokio::test]
async fn deleting_a_tenant_retires_every_connection_in_it() {
	let fixture = fixture(1, FakeAuthorization::default());
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	fixture
		.service
		.retire_tenant("other", "tenant-deletion")
		.await
		.unwrap();
	assert_eq!(fixture.repo.0.lock().await.rows.len(), 1);
	fixture
		.service
		.retire_tenant(TENANT, "tenant-deletion")
		.await
		.unwrap();
	assert!(fixture.repo.0.lock().await.rows.is_empty());
	assert_eq!(fixture.store.resources(), 0);
	assert_eq!(fixture.authorization.calls.lock().unwrap().revoked.len(), 1);
}

#[tokio::test]
async fn usability_checks_read_no_tokens() {
	let fixture = fixture(1, FakeAuthorization::default());
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	let pin = only_row(&fixture).await.pin();
	fixture.service.usable(pin).await.unwrap();
	set_row(&fixture, |row| row.access_expires_at = Utc::now()).await;
	// An expired access token is refreshed by a call, not by a check.
	fixture.service.usable(pin).await.unwrap();
	assert_eq!(fixture.authorization.calls.lock().unwrap().refreshes, 0);
	fixture.repo.0.lock().await.mapped = mapped(0);
	assert!(matches!(
		fixture.service.usable(pin).await,
		Err(Error::Plan(Failure::NotConnected))
	));
	fixture.repo.0.lock().await.mapped = mapped(1);
	set_row(&fixture, |row| row.state = State::Ineligible).await;
	assert!(matches!(
		fixture.service.usable(pin).await,
		Err(Error::Plan(Failure::Ineligible))
	));
	assert!(matches!(
		fixture
			.service
			.usable(Pin {
				connection: Uuid::now_v7(),
				identity: IDENTITY,
			})
			.await,
		Err(Error::Plan(Failure::NotConnected))
	));
}

#[tokio::test]
async fn unreadable_tokens_ask_for_reconnection() {
	let fixture = fixture(1, FakeAuthorization::default());
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	let pin = only_row(&fixture).await.pin();
	fixture
		.service
		.mark_unreadable(pin.connection, true, "secret-store-recovery")
		.await
		.unwrap();
	let row = only_row(&fixture).await;
	assert_eq!(row.state, State::NeedsReconnection);
	assert!(matches!(
		fixture.service.bearer(pin).await,
		Err(Error::Plan(Failure::NeedsReconnection))
	));
	// A second recovery run changes nothing.
	let revision = row.revision;
	fixture
		.service
		.mark_unreadable(pin.connection, true, "secret-store-recovery")
		.await
		.unwrap();
	assert_eq!(only_row(&fixture).await.revision, revision);
	// The Store removed the lost version, so the grant could not be revoked
	// at OpenAI, and disconnecting says so.
	let lost = row.pinned_version.clone().expect("the lost pin is kept");
	fixture.store.destroy(TENANT, &lost).await.unwrap();
	let result = fixture
		.service
		.disconnect(&owner(), revision, "alice")
		.await
		.unwrap();
	assert!(!result.revoked);
	assert!(
		fixture
			.authorization
			.calls
			.lock()
			.unwrap()
			.revoked
			.is_empty()
	);
}

#[tokio::test]
async fn model_lists_use_the_owners_own_connection() {
	let fixture = fixture(1, FakeAuthorization::default());
	assert!(matches!(
		fixture.service.models(&owner()).await,
		Err(Error::Plan(Failure::NotConnected))
	));
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	let models = fixture.service.models(&owner()).await.unwrap();
	assert_eq!(models[0].slug, "gpt-fixture");
}

/// RFC 6749 §5.1: a token response may omit `scope` when it equals the
/// requested one, which includes plan usage.
#[tokio::test]
async fn an_omitted_scope_inherits_the_requested_plan_usage() {
	let fixture = fixture(
		1,
		FakeAuthorization {
			scope: None,
			..FakeAuthorization::default()
		},
	);
	assert_eq!(
		connect(&fixture, None, Some("oaiapp_1")).await.unwrap(),
		Outcome::Connected
	);
	assert!(
		fixture
			.authorization
			.calls
			.lock()
			.unwrap()
			.revoked
			.is_empty()
	);
}

/// A reconnection must authenticate the account the connection was issued
/// for; another account's grant is revoked and nothing changes.
#[tokio::test]
async fn reconnecting_another_chatgpt_account_is_refused() {
	let fixture = fixture(1, FakeAuthorization::default());
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	let before = only_row(&fixture).await;
	assert_eq!(before.account, "user-1");
	*fixture.authorization.account.lock().unwrap() = "user-2".into();
	assert!(matches!(
		connect(&fixture, Some("oaiapp_1"), None).await,
		Err(Error::Conflict(_))
	));
	let after = only_row(&fixture).await;
	assert_eq!(after.pinned_version, before.pinned_version);
	assert_eq!(after.revision, before.revision);
	assert_eq!(
		fixture.authorization.calls.lock().unwrap().revoked,
		vec!["refresh-1"]
	);
}

/// A failed replacement Store write is gracefully revoked and discarded.
/// A metadata commit failure leaves the durable replacement recoverable.
#[rstest::rstest]
#[case::store_write(true, 0, false)]
#[case::unconfirmed_revoke(true, 0, true)]
#[case::metadata_commit(false, 1, false)]
#[tokio::test]
async fn rotated_tokens_are_abandoned_only_before_local_durability(
	#[case] store_fails: bool,
	#[case] failing_commits: usize,
	#[case] revoke_fails: bool,
) {
	let fixture = fixture(
		1,
		FakeAuthorization {
			revoke_fails,
			..FakeAuthorization::default()
		},
	);
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	set_row(&fixture, |row| row.access_expires_at = Utc::now()).await;
	let pin = only_row(&fixture).await.pin();
	{
		let mut faults = fixture.store.1.lock().unwrap();
		faults.add_version = store_fails;
		faults.passing_versions = 1; // intent durability succeeds
	}
	fixture.repo.0.lock().await.failing_commits = failing_commits;
	let result = fixture.service.bearer(pin).await;
	if store_fails {
		assert!(matches!(
			result,
			Err(Error::Plan(Failure::NeedsReconnection))
		));
		assert_eq!(
			fixture.authorization.calls.lock().unwrap().revoked,
			vec!["refresh-r1"]
		);
		let row = only_row(&fixture).await;
		assert_eq!(row.state, State::NeedsReconnection);
		if revoke_fails {
			assert!(row.pinned_version.is_some());
			assert!(
				!fixture
					.service
					.disconnect(&owner(), row.revision, "alice")
					.await
					.unwrap()
					.revoked
			);
		} else {
			assert!(row.pinned_version.is_none());
		}
		assert!(fixture.store.plaintexts().is_empty());
	} else {
		assert!(matches!(result, Err(Error::External(_))));
		assert_eq!(
			fixture.service.bearer(pin).await.unwrap().expose_secret(),
			"access-r1"
		);
		assert!(
			fixture
				.authorization
				.calls
				.lock()
				.unwrap()
				.revoked
				.is_empty()
		);
		assert_eq!(fixture.authorization.calls.lock().unwrap().refreshes, 1);
	}
}

#[tokio::test]
async fn a_refresh_requires_durable_intent_before_dispatch() {
	let fixture = fixture(1, FakeAuthorization::default());
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	set_row(&fixture, |row| row.access_expires_at = Utc::now()).await;
	let before = only_row(&fixture).await;
	fixture.store.1.lock().unwrap().add_version = true;
	assert!(matches!(
		fixture.service.bearer(before.pin()).await,
		Err(Error::External(_))
	));
	assert_eq!(fixture.authorization.calls.lock().unwrap().refreshes, 0);
	assert_eq!(
		only_row(&fixture).await.pinned_version,
		before.pinned_version
	);
}

#[rstest::rstest]
#[case::disconnect(false, false, false)]
#[case::reconnect(true, false, false)]
#[case::reconnect_then_expire(true, true, false)]
#[case::interrupted_cleanup_then_reconnect(true, false, true)]
#[case::interrupted_expired_cleanup_then_reconnect(true, true, true)]
#[tokio::test]
async fn an_unknown_refresh_result_is_never_replayed_or_reported_revoked(
	#[case] reconnect: bool,
	#[case] expire: bool,
	#[case] interrupt_cleanup: bool,
) {
	let fixture = fixture(
		1,
		FakeAuthorization {
			refresh: RefreshMode::Ambiguous,
			..FakeAuthorization::default()
		},
	);
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	set_row(&fixture, |row| row.access_expires_at = Utc::now()).await;
	let pin = only_row(&fixture).await.pin();
	assert!(matches!(
		fixture.service.bearer(pin).await,
		Err(Error::External(_))
	));
	for _ in 0..2 {
		assert!(matches!(
			fixture.service.bearer(pin).await,
			Err(Error::Plan(Failure::NeedsReconnection))
		));
	}
	assert_eq!(fixture.authorization.calls.lock().unwrap().refreshes, 1);
	if reconnect {
		connect(&fixture, Some("oaiapp_1"), None).await.unwrap();
		assert_eq!(
			fixture.service.bearer(pin).await.unwrap().expose_secret(),
			"access-1"
		);
	}
	if expire {
		set_row(&fixture, |row| row.refresh_expires_at = Utc::now()).await;
		assert!(matches!(
			fixture.service.bearer(pin).await,
			Err(Error::Plan(Failure::NeedsReconnection))
		));
	}
	if interrupt_cleanup {
		// Every version was durably destroyed, but resource deletion fails
		// and the independent metadata transaction rolls back. Reconnecting
		// must not overwrite the remaining evidence: its missing old pin.
		fixture.store.1.lock().unwrap().delete = true;
		let row = only_row(&fixture).await;
		assert!(
			fixture
				.service
				.disconnect(&owner(), row.revision, "alice")
				.await
				.is_err()
		);
		assert!(fixture.store.plaintexts().is_empty());
		fixture.store.1.lock().unwrap().delete = false;
		connect(&fixture, Some("oaiapp_1"), None).await.unwrap();
	}
	let row = only_row(&fixture).await;
	assert!(
		!fixture
			.service
			.disconnect(&owner(), row.revision, "alice")
			.await
			.unwrap()
			.revoked
	);
	assert!(fixture.store.plaintexts().is_empty());
}

/// `/models` can reject an access token before its recorded expiry; one
/// forced refresh lets the owner list models without reconnecting.
#[tokio::test]
async fn model_lists_refresh_once_after_an_early_rejection() {
	let fixture = fixture(1, FakeAuthorization::default());
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	*fixture.authorization.models_reject.lock().unwrap() = 1;
	let models = fixture.service.models(&owner()).await.unwrap();
	assert_eq!(models[0].slug, "gpt-fixture");
	assert_eq!(fixture.authorization.calls.lock().unwrap().refreshes, 1);
}

/// The row is the only pointer to the Plan Tokens: a failed cleanup keeps it,
/// so a retried disconnect finishes the deletion.
#[tokio::test]
async fn failed_token_cleanup_keeps_the_connection_for_a_retry() {
	let fixture = fixture(1, FakeAuthorization::default());
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	fixture.store.1.lock().unwrap().delete = true;
	assert!(
		fixture
			.service
			.disconnect(&owner(), 2, "alice")
			.await
			.is_err()
	);
	assert_eq!(only_row(&fixture).await.revision, 2);
	fixture.store.1.lock().unwrap().delete = false;
	fixture
		.service
		.disconnect(&owner(), 2, "alice")
		.await
		.unwrap();
	assert!(fixture.repo.0.lock().await.rows.is_empty());
	assert_eq!(fixture.store.resources(), 0);
}

/// A callback is bound to the attempt whose `state` it returns; another
/// attempt's callback, even a denial, is refused before any exchange.
#[tokio::test]
async fn callbacks_from_another_attempt_are_refused() {
	let fixture = fixture(1, FakeAuthorization::default());
	let verifier = SecretString::from("verifier-1".to_owned());
	for result in [
		CallbackResult::Denied,
		CallbackResult::Code {
			code: "code-1".into(),
			client_id: Some("oaiapp_1".into()),
		},
	] {
		let outcome = fixture
			.service
			.complete(
				&owner(),
				IDENTITY,
				Pending {
					state: "state-2",
					verifier: &verifier,
					nonce: "nonce-1",
					redirect_uri: "http://127.0.0.1:43123/auth/callback",
					saved_client: None,
				},
				&Callback {
					state: "state-1".into(),
					result,
				},
				"alice",
			)
			.await;
		assert!(matches!(outcome, Err(Error::Invalid(_))), "{outcome:?}");
	}
	assert_eq!(fixture.authorization.calls.lock().unwrap().exchanges, 0);
	assert_eq!(fixture.store.resources(), 0);
}

/// A reconnect that completes while an unusable rotation is abandoned keeps
/// its fresh grant: only the state whose token was spent is discarded.
#[tokio::test]
async fn abandoning_a_rotation_preserves_a_concurrent_reconnect() {
	let fixture = fixture(1, FakeAuthorization::default());
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	set_row(&fixture, |row| row.access_expires_at = Utc::now()).await;
	let pin = only_row(&fixture).await.pin();
	{
		let mut faults = fixture.store.1.lock().unwrap();
		faults.add_version = true;
		faults.passing_versions = 1;
	}
	*fixture
		.authorization
		.reconnect_during_revoke
		.lock()
		.unwrap() = Some(fixture.repo.clone());
	assert!(fixture.service.bearer(pin).await.is_err());
	let row = only_row(&fixture).await;
	assert_eq!(row.state, State::Active);
	assert_eq!(row.revision, 3);
	assert!(row.pinned_version.unwrap().ends_with("/versions/99"));
}

/// The connection row is written before any token, so tokens a first
/// connection could not pin stay reachable: a disconnect deletes them.
#[rstest::rstest]
#[case::store_write(true, 0)]
#[case::metadata_commit(false, 1)]
#[tokio::test]
async fn failed_first_connections_leave_a_row_that_finds_their_tokens(
	#[case] store_fails: bool,
	#[case] failing_commits: usize,
) {
	let fixture = fixture(1, FakeAuthorization::default());
	fixture.store.1.lock().unwrap().add_version = store_fails;
	fixture.store.1.lock().unwrap().destroy = true;
	{
		// The registration commits; pinning the tokens does not.
		let mut data = fixture.repo.0.lock().await;
		data.passing_commits = 1;
		data.failing_commits = failing_commits;
	}
	assert!(connect(&fixture, None, Some("oaiapp_1")).await.is_err());
	let row = only_row(&fixture).await;
	assert_eq!(row.state, State::NeedsReconnection);
	assert!(row.pinned_version.is_none());
	fixture.store.1.lock().unwrap().add_version = false;
	fixture.store.1.lock().unwrap().destroy = false;
	fixture
		.service
		.disconnect(&owner(), row.revision, "alice")
		.await
		.unwrap();
	assert!(fixture.store.plaintexts().is_empty());
	assert_eq!(fixture.store.resources(), 0);
}

/// Discarded tokens that the Store could not destroy keep the connection as
/// it was, so the next call retries the transition instead of losing them.
#[tokio::test]
async fn failed_discards_are_retried_by_the_next_call() {
	let fixture = fixture(
		1,
		FakeAuthorization {
			refresh: RefreshMode::Terminal,
			..FakeAuthorization::default()
		},
	);
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	set_row(&fixture, |row| row.access_expires_at = Utc::now()).await;
	let pin = only_row(&fixture).await.pin();
	fixture.store.1.lock().unwrap().destroy = true;
	assert!(matches!(
		fixture.service.bearer(pin).await,
		Err(Error::External(_))
	));
	let row = only_row(&fixture).await;
	assert_eq!(row.state, State::Active);
	assert!(row.pinned_version.is_some());
	assert_eq!(fixture.store.plaintexts().len(), 2);
	fixture.store.1.lock().unwrap().destroy = false;
	assert!(matches!(
		fixture.service.bearer(pin).await,
		Err(Error::Plan(Failure::NeedsReconnection))
	));
	assert_eq!(only_row(&fixture).await.state, State::NeedsReconnection);
	let row = only_row(&fixture).await;
	assert_eq!(fixture.authorization.calls.lock().unwrap().refreshes, 1);
	assert!(
		!fixture
			.service
			.disconnect(&owner(), row.revision, "alice")
			.await
			.unwrap()
			.revoked
	);
	assert!(fixture.store.plaintexts().is_empty());
}

/// A discard that destroyed the tokens but could not record its transition
/// leaves a pin the Store rejects; the next call records the reconnection
/// instead of failing the same read every time.
#[tokio::test]
async fn discards_lost_after_their_cleanup_are_recorded_by_the_next_call() {
	let fixture = fixture(
		1,
		FakeAuthorization {
			refresh: RefreshMode::Terminal,
			..FakeAuthorization::default()
		},
	);
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	set_row(&fixture, |row| row.access_expires_at = Utc::now()).await;
	let pin = only_row(&fixture).await.pin();
	fixture.repo.0.lock().await.failing_commits = 1;
	assert!(matches!(
		fixture.service.bearer(pin).await,
		Err(Error::External(_))
	));
	assert_eq!(only_row(&fixture).await.state, State::Active);
	assert!(fixture.store.plaintexts().is_empty());
	for _ in 0..2 {
		assert!(matches!(
			fixture.service.bearer(pin).await,
			Err(Error::Plan(Failure::NeedsReconnection))
		));
	}
	let row = only_row(&fixture).await;
	assert_eq!(row.state, State::NeedsReconnection);
	assert!(row.pinned_version.is_some());
	assert_eq!(fixture.authorization.calls.lock().unwrap().refreshes, 1);
}

/// A model list that reports an ineligible plan marks the connection, as an
/// inference rejection does.
#[tokio::test]
async fn ineligible_model_lists_mark_the_connection() {
	let fixture = fixture(
		1,
		FakeAuthorization {
			models_ineligible: true,
			..FakeAuthorization::default()
		},
	);
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	assert!(matches!(
		fixture.service.models(&owner()).await,
		Err(Error::Plan(Failure::Ineligible))
	));
	assert_eq!(only_row(&fixture).await.state, State::Ineligible);
}

/// OpenAI asks for a new sign-in only after a terminal refresh error or a
/// confirmed revocation, so a freshly refreshed token that is rejected again
/// is unavailability, not a reason to reconnect.
#[tokio::test]
async fn rejected_refreshed_tokens_are_unavailability() {
	let fixture = fixture(1, FakeAuthorization::default());
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	*fixture.authorization.models_reject.lock().unwrap() = 2;
	assert!(matches!(
		fixture.service.models(&owner()).await,
		Err(Error::External(_))
	));
	assert_eq!(fixture.authorization.calls.lock().unwrap().refreshes, 1);
	assert_eq!(only_row(&fixture).await.state, State::Active);
}

/// A forced refresh waits for OpenAI's `earliest_refresh_at` instead of
/// risking a refused refresh that would end the grant.
#[tokio::test]
async fn forced_refreshes_wait_for_the_earliest_refresh_time() {
	let fixture = fixture(1, FakeAuthorization::default());
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	set_row(&fixture, |row| {
		row.earliest_refresh_at = Some(Utc::now() + Duration::seconds(60));
	})
	.await;
	let pin = only_row(&fixture).await.pin();
	assert!(matches!(
		fixture.service.refresh_rejected(pin).await,
		Err(Error::External(_))
	));
	assert_eq!(fixture.authorization.calls.lock().unwrap().refreshes, 0);
	set_row(&fixture, |row| row.earliest_refresh_at = Some(Utc::now())).await;
	assert!(fixture.service.refresh_rejected(pin).await.unwrap());
	assert_eq!(fixture.authorization.calls.lock().unwrap().refreshes, 1);
}

/// Identities A and B map to alice and B is disabled; A connects. Disabling A
/// and restoring B leaves B the only active identity, yet B never uses A's
/// Plan Tokens (#315).
#[tokio::test]
async fn connections_serve_only_the_identity_that_connected_them() {
	let fixture = fixture(1, FakeAuthorization::default());
	fixture.repo.0.lock().await.mapped.push(MappedIdentity {
		id: OTHER,
		disabled: true,
	});
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	let pin = only_row(&fixture).await.pin();
	assert_eq!(only_row(&fixture).await.identity_id, IDENTITY);
	fixture.service.usable(pin).await.unwrap();
	fixture.repo.0.lock().await.mapped = vec![
		MappedIdentity {
			id: IDENTITY,
			disabled: true,
		},
		MappedIdentity {
			id: OTHER,
			disabled: false,
		},
	];
	assert!(matches!(
		fixture.service.usable(pin).await,
		Err(Error::Plan(Failure::NotConnected))
	));
	assert!(matches!(
		fixture.service.bearer(pin).await,
		Err(Error::Plan(Failure::NotConnected))
	));
	assert!(matches!(
		fixture.service.models(&owner()).await,
		Err(Error::Plan(Failure::NotConnected))
	));
	// Only the identity that connected may start a connection while it is
	// the sole active one.
	assert!(matches!(
		fixture.service.check(&owner(), IDENTITY).await,
		Err(Error::Conflict(_))
	));
	// A reconnection by B, which must authenticate the same ChatGPT account,
	// re-records the connection as B's.
	assert_eq!(
		fixture
			.service
			.check(&owner(), OTHER)
			.await
			.unwrap()
			.as_deref(),
		Some("oaiapp_1")
	);
	assert_eq!(
		connect_as(&fixture, OTHER, Some("oaiapp_1"), None)
			.await
			.unwrap(),
		Outcome::Reconnected
	);
	assert_eq!(only_row(&fixture).await.identity_id, OTHER);
	// A pin admitted for A, even one read just before B reconnected, never
	// reaches B's tokens.
	assert!(matches!(
		fixture.service.usable(pin).await,
		Err(Error::Plan(Failure::NotConnected))
	));
	assert!(matches!(
		fixture.service.bearer(pin).await,
		Err(Error::Plan(Failure::NotConnected))
	));
	assert!(matches!(
		fixture.service.refresh_rejected(pin).await,
		Err(Error::Plan(Failure::NotConnected))
	));
	fixture.service.mark_ineligible(pin).await.unwrap();
	let row = only_row(&fixture).await;
	assert_eq!(row.state, State::Active);
	fixture.service.usable(row.pin()).await.unwrap();
	assert_eq!(
		fixture
			.service
			.bearer(row.pin())
			.await
			.unwrap()
			.expose_secret(),
		"access-1"
	);
}

/// A connection ends once the identity that connected it holds no enabled
/// Mapping to its subject, whatever other identities are mapped there;
/// Identity Disablement alone keeps it.
#[tokio::test]
async fn connections_retire_with_their_owners_mapping() {
	let fixture = fixture(1, FakeAuthorization::default());
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	fixture.repo.0.lock().await.mapped = vec![MappedIdentity {
		id: IDENTITY,
		disabled: true,
	}];
	fixture
		.service
		.retire_unmapped(&owner(), "tenant_administrator")
		.await
		.unwrap();
	assert_eq!(fixture.repo.0.lock().await.rows.len(), 1);
	fixture.repo.0.lock().await.mapped = vec![MappedIdentity {
		id: OTHER,
		disabled: false,
	}];
	fixture
		.service
		.retire_unmapped(&owner(), "tenant_administrator")
		.await
		.unwrap();
	assert!(fixture.repo.0.lock().await.rows.is_empty());
	assert_eq!(fixture.store.resources(), 0);
	assert_eq!(
		fixture.authorization.calls.lock().unwrap().revoked,
		vec!["refresh-1"]
	);
	// Nothing left to retire is not an error.
	fixture
		.service
		.retire_unmapped(&owner(), "tenant_administrator")
		.await
		.unwrap();
}

/// An interrupted rotation's unpinned receipt must not overwrite a reconnect.
/// Its grant remains reachable for revocation through later refresh cleanup.
#[tokio::test]
async fn a_stale_rotation_receipt_never_replaces_a_reconnected_grant() {
	let fixture = fixture(1, FakeAuthorization::default());
	connect(&fixture, None, Some("oaiapp_1")).await.unwrap();
	set_row(&fixture, |row| row.access_expires_at = Utc::now()).await;
	let pin = only_row(&fixture).await.pin();
	fixture.repo.0.lock().await.failing_commits = 1;
	assert!(matches!(
		fixture.service.bearer(pin).await,
		Err(Error::External(_))
	));
	connect(&fixture, Some("oaiapp_1"), None).await.unwrap();
	assert_eq!(
		fixture.service.bearer(pin).await.unwrap().expose_secret(),
		"access-1"
	);
	assert_eq!(fixture.authorization.calls.lock().unwrap().refreshes, 1);
	assert_eq!(only_row(&fixture).await.revision, 3);
	set_row(&fixture, |row| row.access_expires_at = Utc::now()).await;
	fixture.service.bearer(pin).await.unwrap();
	let row = only_row(&fixture).await;
	assert!(
		fixture
			.service
			.disconnect(&owner(), row.revision, "alice")
			.await
			.unwrap()
			.revoked
	);
	assert!(
		fixture
			.authorization
			.calls
			.lock()
			.unwrap()
			.revoked
			.contains(&"refresh-r1".into())
	);
	assert!(fixture.store.plaintexts().is_empty());
}
