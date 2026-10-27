use super::*;
use std::collections::BTreeMap;
use tokio::sync::{Mutex, OwnedMutexGuard};

#[derive(Clone, Default)]
struct Data {
	rows: BTreeMap<Uuid, TenantSecret>,
	events: Vec<(String, i64)>,
}
#[derive(Clone, Default)]
struct Repo(Arc<Mutex<Data>>);
struct Lease {
	guard: OwnedMutexGuard<Data>,
	data: Data,
	tenant: String,
}
#[async_trait]
impl Repository for Repo {
	async fn begin(&self, tenant: &str) -> Result<Box<dyn Scope>> {
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
	) -> Result<Vec<TenantSecret>> {
		Ok(self
			.0
			.lock()
			.await
			.rows
			.values()
			.filter(|r| {
				(r.state != State::Deleted || r.pinned_version.is_some())
					&& after.is_none_or(|id| r.id > id)
			})
			.take(limit)
			.cloned()
			.collect())
	}
	async fn all(&self, tenant: &str) -> Result<Vec<TenantSecret>> {
		Ok(self
			.0
			.lock()
			.await
			.rows
			.values()
			.filter(|r| r.tenant == tenant)
			.cloned()
			.collect())
	}
}
#[async_trait]
impl Scope for Lease {
	async fn named(&mut self, name: &str) -> Result<Option<TenantSecret>> {
		Ok(self
			.data
			.rows
			.values()
			.find(|r| r.tenant == self.tenant && r.name == name && r.state != State::Deleted)
			.cloned())
	}
	async fn by_id(&mut self, id: Uuid) -> Result<Option<TenantSecret>> {
		Ok(self.data.rows.get(&id).cloned())
	}
	async fn list(&mut self) -> Result<Vec<TenantSecret>> {
		Ok(self
			.data
			.rows
			.values()
			.filter(|r| r.tenant == self.tenant && r.state != State::Deleted)
			.cloned()
			.collect())
	}
	async fn count(&mut self) -> Result<usize> {
		Ok(self.list().await?.len())
	}
	async fn insert(&mut self, value: &TenantSecret) -> Result<()> {
		self.data.rows.insert(value.id, value.clone());
		Ok(())
	}
	async fn save(&mut self, value: &TenantSecret, event: &str, _: &str) -> Result<()> {
		self.data.rows.insert(value.id, value.clone());
		self.data.events.push((event.into(), value.revision));
		Ok(())
	}
	async fn remove(&mut self, id: Uuid) -> Result<()> {
		self.data.rows.remove(&id);
		Ok(())
	}
	async fn commit(mut self: Box<Self>) -> Result<()> {
		*self.guard = self.data.clone();
		Ok(())
	}
}

type SecretVersions = BTreeMap<String, BTreeMap<String, (String, &'static str)>>;
type SealedHosts = BTreeMap<String, BTreeMap<String, Vec<String>>>;

#[derive(Clone, Default)]
struct FakeStore {
	/// resource -> version -> (value, state)
	state: Arc<Mutex<SecretVersions>>,
	/// What each version sealed: resource -> version -> hosts.
	sealed_hosts: Arc<Mutex<SealedHosts>>,
	refuse_secrets: bool,
	/// Whether this Store seals the hosts with the value (ADR 0096).
	seals: bool,
}
#[async_trait]
impl Store for FakeStore {
	fn resource(&self, id: Uuid) -> String {
		format!("fake/provider/{id}")
	}
	async fn create(&self, _: &str, _: Uuid) -> Result<()> {
		unreachable!("Tenant Secrets never use the Provider Credential namespace")
	}
	async fn add_version(&self, _: &str, resource: &str, key: &SecretString) -> Result<String> {
		let mut state = self.state.lock().await;
		let versions = state.get_mut(resource).unwrap();
		let name = format!("{resource}/versions/{}", versions.len() + 1);
		versions.insert(name.clone(), (key.expose_secret().to_owned(), "enabled"));
		Ok(name)
	}
	async fn versions(&self, _: &str, resource: &str) -> Result<Vec<String>> {
		Ok(self
			.state
			.lock()
			.await
			.get(resource)
			.map(|v| v.keys().cloned().collect())
			.unwrap_or_default())
	}
	async fn disable(&self, _: &str, version: &str) -> Result<()> {
		for versions in self.state.lock().await.values_mut() {
			if let Some(v) = versions.get_mut(version) {
				v.1 = "disabled";
			}
		}
		Ok(())
	}
	async fn destroy(&self, _: &str, version: &str) -> Result<()> {
		for versions in self.state.lock().await.values_mut() {
			if let Some(v) = versions.get_mut(version) {
				v.0.clear();
				v.1 = "destroyed";
			}
		}
		Ok(())
	}
	async fn delete(&self, _: &str, resource: &str) -> Result<()> {
		self.state.lock().await.remove(resource);
		Ok(())
	}
	async fn create_secret(&self, _: &str, id: Uuid) -> Result<()> {
		if self.refuse_secrets {
			return Err(Error::Invalid(
				"this Secret Store does not hold Tenant Secrets".into(),
			));
		}
		self.state
			.lock()
			.await
			.insert(self.secret_resource(id), BTreeMap::new());
		Ok(())
	}
	fn seals_secret_hosts(&self) -> bool {
		self.seals
	}
	async fn add_secret_version(
		&self,
		tenant: &str,
		resource: &str,
		hosts: &[String],
		value: &SecretString,
	) -> Result<String> {
		if !self.seals {
			return self.add_version(tenant, resource, value).await;
		}
		let sealed = aidash_capability::tenant_secret::sealed::seal(hosts, value.expose_secret())
			.map_err(|_| Error::Invalid("invalid Tenant Secret hosts".into()))?;
		let payload = SecretString::from(String::from_utf8_lossy(&sealed).into_owned());
		let version = self.add_version(tenant, resource, &payload).await?;
		self.sealed_hosts
			.lock()
			.await
			.entry(resource.to_owned())
			.or_default()
			.insert(version.clone(), hosts.to_vec());
		Ok(version)
	}
}
#[async_trait]
impl KeyMaterialReader for FakeStore {
	async fn read(&self, _: &str, resource: &str, version: &str) -> Result<SecretString> {
		match self
			.state
			.lock()
			.await
			.get(resource)
			.and_then(|v| v.get(version))
		{
			Some((value, "enabled")) => Ok(SecretString::from(value.clone())),
			_ => Err(Error::Invalid("cannot read the pinned version".into())),
		}
	}
}

fn service_with(store: FakeStore) -> (Service, Repo, FakeStore) {
	let repo = Repo::default();
	(
		Service {
			repository: Arc::new(repo.clone()),
			store: Arc::new(store.clone()),
			reader: Some(Arc::new(store.clone())),
			calls: None,
			fingerprint_key: "test-fingerprint-root-key".into(),
			max_per_tenant: 2,
		},
		repo,
		store,
	)
}

#[derive(Clone, Default)]
struct RecordedGrant(Arc<Mutex<Vec<SecretCallGrant>>>);
struct FakeCalls(RecordedGrant);
#[async_trait]
impl SecretCalls for FakeCalls {
	async fn route(&self, grant: SecretCallGrant) -> Result<SecretRoute> {
		self.0.0.lock().await.push(grant);
		Ok(SecretRoute {
			base: Url::parse("https://broker.example/v1/forward").unwrap(),
			token: "canary-capability-token".into(),
		})
	}
}

fn service_with_calls(store: FakeStore) -> (Service, RecordedGrant, FakeStore) {
	let repo = Repo::default();
	let grants = RecordedGrant::default();
	(
		Service {
			repository: Arc::new(repo),
			store: Arc::new(store.clone()),
			// A brokered Store is write-only: this reader must never be called.
			reader: None,
			calls: Some(Arc::new(FakeCalls(grants.clone()))),
			fingerprint_key: "test-fingerprint-root-key".into(),
			max_per_tenant: 2,
		},
		grants,
		store,
	)
}
fn service() -> (Service, Repo, FakeStore) {
	service_with(FakeStore::default())
}
fn put(value: Option<&str>, hosts: Option<&[&str]>, expected: i64) -> Put {
	Put {
		expected_revision: expected,
		value: value.map(|v| SecretString::from(v.to_owned())),
		hosts: hosts.map(|h| h.iter().map(|h| (*h).to_owned()).collect()),
	}
}
fn invalid<T>(result: &Result<T>) -> bool {
	matches!(
		result,
		Err(Error::Invalid(_) | Error::Domain(aidash_domain::Error::Invalid(_)))
	)
}
fn conflict<T>(result: &Result<T>) -> bool {
	matches!(
		result,
		Err(Error::Conflict(_) | Error::Domain(aidash_domain::Error::Conflict(_)))
	)
}
const ENDPOINT: &str = "https://api.example.com/tool";

#[tokio::test]
async fn a_stored_secret_shows_only_a_fingerprint_and_last_four_and_resolves_to_its_hosts() {
	let (s, _, _) = service();
	let created = s
		.put(
			"alpha",
			"search",
			put(
				Some("super-secret-value-1234"),
				Some(&["API.example.com"]),
				0,
			),
			"actor",
		)
		.await
		.unwrap();
	assert_eq!(created.last4, "1234");
	assert_eq!(created.hosts, ["api.example.com"]);
	assert_eq!(created.revision, 2);
	let shown = serde_json::to_string(&created).unwrap();
	assert!(!shown.contains("super-secret-value"), "{shown}");
	let value = s.resolve_value("alpha", "search", ENDPOINT).await.unwrap();
	assert_eq!(value.expose_secret(), "super-secret-value-1234");
	// Another host, another Tenant and a missing name all fail the same way.
	for (tenant, name, endpoint) in [
		("alpha", "search", "https://evil.example/tool"),
		("beta", "search", ENDPOINT),
		("alpha", "other", ENDPOINT),
	] {
		assert!(matches!(
			s.resolve_value(tenant, name, endpoint).await,
			Err(Error::Invalid(m)) if m == UNAVAILABLE
		));
	}
}

#[tokio::test]
async fn replacing_the_value_keeps_the_name_and_disables_the_old_version() {
	let (s, _, store) = service();
	let first = s
		.put(
			"alpha",
			"search",
			put(Some("first-value-aaaa"), Some(&["api.example.com"]), 0),
			"actor",
		)
		.await
		.unwrap();
	let second = s
		.put(
			"alpha",
			"search",
			put(Some("second-value-bbbb"), None, first.revision),
			"actor",
		)
		.await
		.unwrap();
	assert_eq!(second.id, first.id);
	assert_eq!(second.last4, "bbbb");
	assert_ne!(second.fingerprint, first.fingerprint);
	assert_eq!(
		s.resolve_value("alpha", "search", ENDPOINT)
			.await
			.unwrap()
			.expose_secret(),
		"second-value-bbbb"
	);
	let state = store.state.lock().await;
	let versions = state.get(&store.secret_resource(first.id)).unwrap();
	assert_eq!(versions.len(), 2);
	assert_eq!(versions.values().filter(|v| v.1 == "enabled").count(), 1);
	drop(state);
	// A stale revision is refused and a no-op request is invalid.
	let stale = s
		.put(
			"alpha",
			"search",
			put(Some("third-value-cccc"), None, first.revision),
			"actor",
		)
		.await;
	assert!(conflict(&stale), "{stale:?}");
	assert!(invalid(
		&s.put("alpha", "search", put(None, None, second.revision), "actor")
			.await
	));
}

#[tokio::test]
async fn changing_hosts_takes_effect_at_the_next_resolution() {
	let (s, _, _) = service();
	let created = s
		.put(
			"alpha",
			"search",
			put(Some("value-1234"), Some(&["api.example.com"]), 0),
			"actor",
		)
		.await
		.unwrap();
	s.put(
		"alpha",
		"search",
		put(None, Some(&["other.example"]), created.revision),
		"actor",
	)
	.await
	.unwrap();
	assert!(s.resolve_value("alpha", "search", ENDPOINT).await.is_err());
	assert_eq!(
		s.hosts("alpha", "search").await.unwrap().unwrap(),
		["other.example"]
	);
}

#[tokio::test]
async fn deleting_makes_calls_fail_destroys_the_value_and_frees_the_name() {
	let (s, repo, store) = service();
	let created = s
		.put(
			"alpha",
			"search",
			put(Some("value-1234"), Some(&["api.example.com"]), 0),
			"actor",
		)
		.await
		.unwrap();
	s.delete("alpha", "search", created.revision, "actor")
		.await
		.unwrap();
	assert!(matches!(
		s.resolve_value("alpha", "search", ENDPOINT).await,
		Err(Error::Invalid(m)) if m == UNAVAILABLE
	));
	assert!(
		store.state.lock().await.is_empty(),
		"the value is destroyed"
	);
	assert!(
		repo.0
			.lock()
			.await
			.rows
			.values()
			.all(|r| r.pinned_version.is_none())
	);
	assert!(s.hosts("alpha", "search").await.unwrap().is_none());
	// The name can be used again.
	s.put(
		"alpha",
		"search",
		put(Some("again-5678"), Some(&["api.example.com"]), 0),
		"actor",
	)
	.await
	.unwrap();
	assert_eq!(s.list("alpha").await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_new_secret_needs_a_value_hosts_a_valid_name_and_room() {
	let (s, _, _) = service();
	for (name, input) in [
		("search", put(None, Some(&["a.example"]), 0)),
		("search", put(Some("value-1234"), None, 0)),
		(
			"search",
			put(Some("value-1234"), Some(&["https://a.example"]), 0),
		),
		("Search", put(Some("value-1234"), Some(&["a.example"]), 0)),
		("search", put(Some(""), Some(&["a.example"]), 0)),
		("search", put(Some(" padded "), Some(&["a.example"]), 0)),
	] {
		let result = s.put("alpha", name, input, "actor").await;
		assert!(invalid(&result), "{name}: {result:?}");
	}
	assert!(conflict(
		&s.put(
			"alpha",
			"search",
			put(Some("value-1234"), Some(&["a.example"]), 3),
			"actor"
		)
		.await
	));
	for name in ["one", "two"] {
		s.put(
			"alpha",
			name,
			put(Some("value-1234"), Some(&["a.example"]), 0),
			"actor",
		)
		.await
		.unwrap();
	}
	assert!(conflict(
		&s.put(
			"alpha",
			"three",
			put(Some("value-1234"), Some(&["a.example"]), 0),
			"actor"
		)
		.await
	));
}

#[tokio::test]
async fn fingerprints_differ_between_tenants_and_events_carry_no_value() {
	let (s, repo, _) = service();
	let a = s
		.put(
			"alpha",
			"search",
			put(Some("same-value-1234"), Some(&["a.example"]), 0),
			"actor",
		)
		.await
		.unwrap();
	let b = s
		.put(
			"beta",
			"search",
			put(Some("same-value-1234"), Some(&["a.example"]), 0),
			"actor",
		)
		.await
		.unwrap();
	assert_ne!(a.fingerprint, b.fingerprint);
	assert_eq!(repo.0.lock().await.events.len(), 2);
}

#[tokio::test]
async fn a_store_that_cannot_hold_tenant_secrets_refuses_and_leaves_nothing_usable() {
	let (s, repo, _) = service_with(FakeStore {
		refuse_secrets: true,
		..Default::default()
	});
	let result = s
		.put(
			"alpha",
			"search",
			put(Some("value-1234"), Some(&["a.example"]), 0),
			"actor",
		)
		.await;
	assert!(matches!(result, Err(Error::Invalid(_))));
	assert!(s.resolve_value("alpha", "search", ENDPOINT).await.is_err());
	// The unfinished row expires and is cleaned up by reconciliation.
	{
		let mut data = repo.0.lock().await;
		for row in data.rows.values_mut() {
			row.created_at = Utc::now() - Duration::minutes(10);
		}
	}
	assert_eq!(s.reconcile().await.unwrap(), 1);
	assert!(s.list("alpha").await.unwrap().is_empty());
}

#[tokio::test]
async fn reconciliation_finishes_an_interrupted_delete_and_purge_removes_everything() {
	let (s, repo, store) = service();
	let created = s
		.put(
			"alpha",
			"search",
			put(Some("value-1234"), Some(&["a.example"]), 0),
			"actor",
		)
		.await
		.unwrap();
	s.put(
		"alpha",
		"other",
		put(Some("value-5678"), Some(&["a.example"]), 0),
		"actor",
	)
	.await
	.unwrap();
	// A crash after the delete committed but before the Store was cleaned.
	{
		let mut data = repo.0.lock().await;
		let row = data.rows.get_mut(&created.id).unwrap();
		row.state = State::Deleted;
	}
	assert_eq!(s.reconcile().await.unwrap(), 1);
	assert!(
		!store
			.state
			.lock()
			.await
			.contains_key(&store.secret_resource(created.id))
	);
	assert_eq!(s.purge_tenant("alpha").await.unwrap(), 2);
	assert!(store.state.lock().await.is_empty());
	assert!(repo.0.lock().await.rows.is_empty());
}

#[test]
fn admission_refuses_a_missing_secret_and_an_unrecorded_host() {
	let hosts = ["api.example.com".to_owned()];
	assert!(admit("search", ENDPOINT, Some(&hosts)).is_ok());
	assert!(admit("search", ENDPOINT, None).is_err());
	assert!(admit("search", "https://evil.example/", Some(&hosts)).is_err());
	assert!(admit("search", "http://api.example.com/", Some(&hosts)).is_err());
}

/// `aidash-capability` is a leaf crate, so its sealed-payload codec repeats the
/// host rule. This crate sees both and keeps them in step.
#[test]
fn sealed_payload_hosts_match_the_domain_normalization() {
	use aidash_capability::tenant_secret::sealed;
	assert_eq!(sealed::MAX_HOSTS, aidash_domain::tenant_secrets::MAX_HOSTS);
	for hosts in [
		vec![
			"API.Example.com".to_owned(),
			"api.example.com".to_owned(),
			"b.example".to_owned(),
		],
		vec!["127.0.0.1".to_owned()],
		vec!["xn--bcher-kva.example".to_owned()],
		vec!["a.example".to_owned(), "b.example".to_owned()],
	] {
		let expected = aidash_domain::tenant_secrets::normalize_hosts(&hosts).unwrap();
		let opened = sealed::open(&sealed::seal(&hosts, "canary-value").unwrap()).unwrap();
		assert_eq!(opened.hosts, expected, "{hosts:?}");
	}
	for bad in [
		"https://a.example",
		"a.example/path",
		"a.example:8443",
		"*.example",
		"user@a.example",
		"",
	] {
		assert!(
			aidash_domain::tenant_secrets::normalize_hosts(&[bad.to_owned()]).is_err(),
			"{bad}"
		);
		assert!(
			sealed::seal(&[bad.to_owned()], "canary-value").is_err(),
			"{bad}"
		);
		let payload = serde_json::json!({"v": 1, "hosts": [bad], "value": "canary-value"});
		assert!(
			sealed::open(&serde_json::to_vec(&payload).unwrap()).is_err(),
			"{bad}"
		);
	}
}

#[tokio::test]
async fn a_sealing_store_refuses_a_hosts_change_without_the_value_and_seals_a_new_version() {
	let (s, repo, store) = service_with(FakeStore {
		seals: true,
		..Default::default()
	});
	let created = s
		.put(
			"alpha",
			"search",
			put(Some("first-value-aaaa"), Some(&["api.example.com"]), 0),
			"actor",
		)
		.await
		.unwrap();
	// The first version seals the hosts with the value.
	let resource = store.secret_resource(created.id);
	let first = {
		let state = store.state.lock().await;
		state
			.get(&resource)
			.unwrap()
			.values()
			.next()
			.unwrap()
			.0
			.clone()
	};
	let opened = aidash_capability::tenant_secret::sealed::open(first.as_bytes()).unwrap();
	assert_eq!(opened.hosts, ["api.example.com"]);
	assert_eq!(opened.value.expose_secret(), "first-value-aaaa");
	// A hosts change without the value is refused and changes nothing.
	let refused = s
		.put(
			"alpha",
			"search",
			put(None, Some(&["other.example"]), created.revision),
			"actor",
		)
		.await;
	assert!(invalid(&refused), "{refused:?}");
	assert!(
		refused
			.unwrap_err()
			.to_string()
			.contains("requires its value"),
		"a clear invalid-input error"
	);
	let unchanged = s.get("alpha", "search").await.unwrap();
	assert_eq!(unchanged.hosts, ["api.example.com"]);
	assert_eq!(unchanged.revision, created.revision);
	// The value and the hosts together write one new sealed version, and the
	// pin follows it.
	let changed = s
		.put(
			"alpha",
			"search",
			put(
				Some("second-value-bbbb"),
				Some(&["Other.Example"]),
				created.revision,
			),
			"actor",
		)
		.await
		.unwrap();
	assert_eq!(changed.hosts, ["other.example"]);
	let row = repo.0.lock().await.rows.get(&created.id).cloned().unwrap();
	let pinned = row.pinned_version.clone().unwrap();
	assert_eq!(pinned, format!("{resource}/versions/2"));
	assert_eq!(
		store.sealed_hosts.lock().await[&resource][&pinned],
		["other.example"]
	);
	let payload = {
		let state = store.state.lock().await;
		state.get(&resource).unwrap()[&pinned].0.clone()
	};
	let opened = aidash_capability::tenant_secret::sealed::open(payload.as_bytes()).unwrap();
	assert_eq!(opened.hosts, ["other.example"]);
	assert_eq!(opened.value.expose_secret(), "second-value-bbbb");
	// Replacing only the value keeps the current hosts in the sealed version.
	let replaced = s
		.put(
			"alpha",
			"search",
			put(Some("third-value-cccc"), None, changed.revision),
			"actor",
		)
		.await
		.unwrap();
	assert_eq!(replaced.hosts, ["other.example"]);
	let row = repo.0.lock().await.rows.get(&created.id).cloned().unwrap();
	let pinned = row.pinned_version.unwrap();
	assert_eq!(
		store.sealed_hosts.lock().await[&resource][&pinned],
		["other.example"]
	);
}

#[tokio::test]
async fn brokered_resolution_returns_a_route_and_never_reads_the_value() {
	let (s, grants, _) = service_with_calls(FakeStore {
		seals: true,
		..Default::default()
	});
	let created = s
		.put(
			"alpha",
			"search",
			put(Some("canary-value-1234"), Some(&["api.example.com"]), 0),
			"actor",
		)
		.await
		.unwrap();
	let call = SecretCall {
		destination: Destination {
			origin: "https://api.example.com".into(),
			path_prefix: "/tool/".into(),
		},
		methods: vec!["POST".into()],
		attach: Attach::Header {
			name: "x-api-key".into(),
		},
		subject: SecretSubject::Run {
			run: "run-1".into(),
			call: Uuid::from_u128(1),
		},
		max_response_bytes: Some(262_144),
	};
	let resolved = s
		.resolve("alpha", "search", ENDPOINT, Some(call))
		.await
		.unwrap();
	let Resolved::Brokered(route) = resolved else {
		panic!("a brokered resolution returns a route");
	};
	assert_eq!(route.base.as_str(), "https://broker.example/v1/forward");
	assert_eq!(route.token.expose_secret(), "canary-capability-token");
	{
		let recorded = grants.0.lock().await;
		assert_eq!(recorded.len(), 1);
		assert_eq!(recorded[0].tenant, "alpha");
		assert_eq!(recorded[0].secret, created.id);
		assert_eq!(recorded[0].version, "1");
		assert_eq!(recorded[0].destination.origin, "https://api.example.com");
		assert_eq!(recorded[0].methods, ["POST"]);
		assert_eq!(recorded[0].max_response_bytes, Some(262_144));
	}
	// The direct path refuses rather than reading: this Service has no reader.
	assert!(
		s.resolve_value("alpha", "search", ENDPOINT)
			.await
			.unwrap_err()
			.to_string()
			.contains("broker")
	);
	// A destination the metadata does not record is refused before a token.
	let mut foreign = SecretCall {
		destination: Destination {
			origin: "https://evil.example".into(),
			path_prefix: "/".into(),
		},
		methods: vec!["GET".into()],
		attach: Attach::Bearer,
		subject: SecretSubject::Run {
			run: "run-1".into(),
			call: Uuid::from_u128(2),
		},
		max_response_bytes: None,
	};
	assert!(matches!(
		s.resolve("alpha", "search", ENDPOINT, Some(foreign.clone()))
			.await,
		Err(Error::Invalid(message)) if message == UNAVAILABLE
	));
	foreign.destination.origin = "https://api.example.com".into();
	assert_eq!(grants.0.lock().await.len(), 1, "no second token was minted");
}

/// The signed destination is the endpoint path's directory, never a sibling.
#[test]
fn an_endpoint_signs_its_directory_and_forwards_only_the_remaining_path() {
	for (endpoint, prefix, rest) in [
		("https://api.example.com/tool", "/", "tool"),
		("https://api.example.com/v1/tools/run", "/v1/tools/", "run"),
		("https://api.example.com/v1/tools/", "/v1/tools/", ""),
		("https://api.example.com/", "/", ""),
	] {
		let destination = endpoint_destination(endpoint).unwrap();
		assert_eq!(destination.origin, "https://api.example.com");
		assert_eq!(destination.path_prefix, prefix, "{endpoint}");
		assert_eq!(forward_rest(endpoint).unwrap(), rest, "{endpoint}");
	}
}

#[tokio::test]
async fn a_brokered_call_secret_records_metadata_and_refreshes_one_route() {
	let (s, grants, _) = service_with_calls(FakeStore {
		seals: true,
		..Default::default()
	});
	let created = s
		.put(
			"alpha",
			"search",
			put(Some("canary-value-1234"), Some(&["api.example.com"]), 0),
			"actor",
		)
		.await
		.unwrap();
	let service = Arc::new(s);
	let secret = service
		.call_secret(
			"alpha",
			"search",
			ENDPOINT,
			SecretCall {
				destination: endpoint_destination(ENDPOINT).unwrap(),
				methods: vec!["POST".into()],
				attach: Attach::Bearer,
				subject: SecretSubject::Run {
					run: "run-1".into(),
					call: Uuid::from_u128(1),
				},
				max_response_bytes: Some(262_144),
			},
		)
		.await
		.unwrap();
	let CallSecret::Brokered {
		record,
		source,
		rest,
	} = &secret
	else {
		panic!("a brokered call returns a route source");
	};
	assert_eq!(record.id, created.id);
	assert_eq!(record.version, "1");
	assert_eq!(record.host, "api.example.com");
	assert_eq!(rest, "tool");
	// The route minted for the call is used first, without another token.
	assert_eq!(
		source.route().await.unwrap().token.expose_secret(),
		"canary-capability-token"
	);
	assert_eq!(grants.0.lock().await.len(), 1);
	// A second request after the token's cache window re-routes through the
	// broker adapter, which re-checks the metadata and mints another token.
	assert_eq!(
		source.route().await.unwrap().token.expose_secret(),
		"canary-capability-token"
	);
	assert_eq!(grants.0.lock().await.len(), 2);
}

#[tokio::test]
async fn a_self_hosted_call_secret_returns_the_value_and_records_metadata() {
	let (s, _, _) = service_with(FakeStore::default());
	let created = s
		.put(
			"alpha",
			"search",
			put(Some("canary-value-1234"), Some(&["api.example.com"]), 0),
			"actor",
		)
		.await
		.unwrap();
	let secret = Arc::new(s)
		.call_secret(
			"alpha",
			"search",
			ENDPOINT,
			SecretCall {
				destination: endpoint_destination(ENDPOINT).unwrap(),
				methods: vec!["POST".into()],
				attach: Attach::Bearer,
				subject: SecretSubject::Workbench {
					tenant: "alpha".into(),
					call: Uuid::from_u128(2),
				},
				max_response_bytes: Some(262_144),
			},
		)
		.await
		.unwrap();
	let CallSecret::Value { record, value } = secret else {
		panic!("a self-hosted call returns the value");
	};
	assert_eq!(record.id, created.id);
	assert_eq!(record.version, "1");
	assert_eq!(record.host, "api.example.com");
	assert_eq!(value.expose_secret(), "canary-value-1234");
}

#[tokio::test]
async fn a_resolution_reports_the_tenant_secret_id_and_pinned_version() {
	let (s, _, _) = service_with_calls(FakeStore {
		seals: true,
		..Default::default()
	});
	let created = s
		.put(
			"alpha",
			"search",
			put(Some("canary-value-1234"), Some(&["api.example.com"]), 0),
			"actor",
		)
		.await
		.unwrap();
	let call = SecretCall {
		destination: Destination {
			origin: "https://api.example.com".into(),
			path_prefix: "/v1/".into(),
		},
		methods: vec!["GET".into()],
		attach: Attach::Bearer,
		subject: SecretSubject::Operation {
			operation: Uuid::from_u128(3),
			tenant: "alpha".into(),
		},
		max_response_bytes: None,
	};
	let resolution = s
		.resolve_with_secret("alpha", "search", ENDPOINT, Some(call))
		.await
		.unwrap();
	assert!(matches!(resolution.resolved, Resolved::Brokered(_)));
	assert_eq!(
		resolution.secret,
		Some(SecretIdentity {
			id: created.id,
			version: "1".into()
		})
	);
}

#[tokio::test(start_paused = true)]
async fn cached_routes_are_reissued_after_fifty_seconds() {
	let cache = RouteCache::default();
	let operation = Uuid::from_u128(7);
	cache.insert(
		operation,
		"images",
		CachedRoute {
			base: Url::parse("https://broker.example/v1/forward").unwrap(),
			token: "first-token".into(),
			secret: SecretIdentity {
				id: Uuid::from_u128(8),
				version: "1".into(),
			},
		},
	);
	assert_eq!(
		cache
			.get(operation, "images")
			.unwrap()
			.token
			.expose_secret(),
		"first-token"
	);
	tokio::time::advance(std::time::Duration::from_secs(49)).await;
	assert!(cache.get(operation, "images").is_some());
	tokio::time::advance(std::time::Duration::from_secs(2)).await;
	assert!(
		cache.get(operation, "images").is_none(),
		"a route is not reused past its 50-second cache lifetime"
	);
}

#[tokio::test]
async fn concurrent_route_cache_access_stays_consistent() {
	let cache = Arc::new(RouteCache::default());
	let operation = Uuid::from_u128(9);
	let workers = (0..8)
		.map(|worker| {
			let cache = cache.clone();
			tokio::spawn(async move {
				for _ in 0..100 {
					let _ = cache.get(operation, "images");
					cache.insert(
						operation,
						"images",
						CachedRoute {
							base: Url::parse("https://broker.example/v1/forward").unwrap(),
							token: format!("token-{worker}").into(),
							secret: SecretIdentity {
								id: Uuid::from_u128(10),
								version: "1".into(),
							},
						},
					);
				}
			})
		})
		.collect::<Vec<_>>();
	for worker in workers {
		worker.await.unwrap();
	}
	assert!(cache.get(operation, "images").is_some());
}

#[tokio::test(start_paused = true)]
async fn a_stale_operation_route_is_minted_again() {
	let (s, grants, _) = service_with_calls(FakeStore {
		seals: true,
		..Default::default()
	});
	s.put(
		"alpha",
		"search",
		put(Some("canary-value-1234"), Some(&["api.example.com"]), 0),
		"actor",
	)
	.await
	.unwrap();
	let call = || SecretCall {
		destination: Destination {
			origin: "https://api.example.com".into(),
			path_prefix: "/v1/".into(),
		},
		methods: vec!["GET".into()],
		attach: Attach::Bearer,
		subject: SecretSubject::Operation {
			operation: Uuid::from_u128(11),
			tenant: "alpha".into(),
		},
		max_response_bytes: None,
	};
	let first = s
		.resolve_with_secret("alpha", "search", ENDPOINT, Some(call()))
		.await
		.unwrap();
	let Resolved::Brokered(route) = first.resolved else {
		panic!("a brokered resolution returns a route");
	};
	let cache = RouteCache::default();
	cache.insert(
		Uuid::from_u128(11),
		"images",
		CachedRoute {
			base: route.base,
			token: route.token,
			secret: first.secret.unwrap(),
		},
	);
	assert!(cache.get(Uuid::from_u128(11), "images").is_some());
	tokio::time::advance(std::time::Duration::from_secs(50)).await;
	assert!(
		cache.get(Uuid::from_u128(11), "images").is_none(),
		"the gateway re-routes once the cached route is 50 seconds old"
	);
	let second = s
		.resolve_with_secret("alpha", "search", ENDPOINT, Some(call()))
		.await
		.unwrap();
	assert!(matches!(second.resolved, Resolved::Brokered(_)));
	assert_eq!(
		grants.0.lock().await.len(),
		2,
		"a fresh token is minted after the cache expires"
	);
}

#[tokio::test]
async fn active_secrets_exclude_pending_and_deleted_names() {
	let (s, _, _) = service_with(FakeStore::default());
	let kept = s
		.put(
			"alpha",
			"kept",
			put(Some("canary-kept-value"), Some(&["api.example.com"]), 0),
			"actor",
		)
		.await
		.unwrap();
	let removed = s
		.put(
			"alpha",
			"removed",
			put(Some("canary-removed-value"), Some(&["api.example.com"]), 0),
			"actor",
		)
		.await
		.unwrap();
	s.delete("alpha", "removed", removed.revision, "actor")
		.await
		.unwrap();
	let active = s.active("alpha").await.unwrap();
	assert_eq!(active.len(), 1);
	assert_eq!(active[0].id, kept.id);
	assert_eq!(active[0].name, "kept");
	assert_eq!(active[0].hosts, ["api.example.com"]);
	assert!(s.active("beta").await.unwrap().is_empty());
}
