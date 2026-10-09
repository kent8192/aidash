use super::*;
use std::collections::BTreeMap;
use tokio::sync::{Mutex, OwnedMutexGuard};
#[derive(Clone, Default)]
struct Data {
	commit_failure: bool,
	events: Vec<(Uuid, String, i64)>,
	rows: BTreeMap<Uuid, ProviderCredential>,
	bindings: BTreeMap<String, Binding>,
}
#[derive(Clone, Default)]
struct Repo(Arc<Mutex<Data>>, Arc<std::sync::atomic::AtomicBool>);
struct Lease {
	guard: OwnedMutexGuard<Data>,
	data: Data,
	tenant: String,
	commit_failure: Arc<std::sync::atomic::AtomicBool>,
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
			commit_failure: self.1.clone(),
		}))
	}
	async fn reconciliation_candidates(
		&self,
		after: Option<Uuid>,
		limit: usize,
	) -> Result<Vec<ProviderCredential>> {
		Ok(self
			.0
			.lock()
			.await
			.rows
			.values()
			.filter(|row| {
				(row.state != State::Deleted || row.pinned_version.is_some())
					&& after.is_none_or(|id| row.id > id)
			})
			.take(limit.clamp(1, RECONCILIATION_BATCH_SIZE))
			.cloned()
			.collect())
	}
}
#[async_trait]
impl Scope for Lease {
	async fn get(&mut self, id: Uuid) -> Result<ProviderCredential> {
		self.data
			.rows
			.get(&id)
			.filter(|r| r.tenant == self.tenant)
			.cloned()
			.ok_or_else(|| Error::NotFound("Provider Credential".into()))
	}
	async fn list(&mut self, offset: usize, limit: usize) -> Result<Vec<ProviderCredential>> {
		Ok(self
			.data
			.rows
			.values()
			.filter(|r| r.tenant == self.tenant)
			.skip(offset)
			.take(limit)
			.cloned()
			.collect())
	}
	async fn count(&mut self) -> Result<usize> {
		Ok(self
			.data
			.rows
			.values()
			.filter(|r| r.tenant == self.tenant && r.state != State::Deleted)
			.count())
	}
	async fn insert(&mut self, row: &ProviderCredential) -> Result<()> {
		assert!(!self.data.rows.contains_key(&row.id));
		self.data.rows.insert(row.id, row.clone());
		Ok(())
	}
	async fn save(&mut self, row: &ProviderCredential, event: &str, _: &str) -> Result<()> {
		assert_eq!(row.tenant, self.tenant);
		self.data.rows.insert(row.id, row.clone());
		self.data.events.push((row.id, event.into(), row.revision));
		Ok(())
	}
	async fn bindings(&mut self) -> Result<Vec<Binding>> {
		Ok(self
			.data
			.bindings
			.values()
			.filter(|b| b.tenant == self.tenant)
			.cloned()
			.collect())
	}
	async fn bind(&mut self, b: &Binding, _: &str) -> Result<()> {
		self.data
			.bindings
			.insert(format!("{}:{}", b.tenant, b.provider.id()), b.clone());
		Ok(())
	}
	async fn commit(mut self: Box<Self>) -> Result<()> {
		if self.guard.commit_failure
			|| self
				.commit_failure
				.load(std::sync::atomic::Ordering::SeqCst)
		{
			return Err(Error::External("database commit failed".into()));
		}
		*self.guard = self.data.clone();
		Ok(())
	}
}
#[derive(Clone, Default)]
struct FakeStore {
	state: Arc<Mutex<StoredSecrets>>,
	disable_failure: Arc<Mutex<Option<String>>>,
	version_lists: Arc<Mutex<Vec<String>>>,
	destroy_failure: Arc<Mutex<Option<String>>>,
	delete_failure: Arc<Mutex<bool>>,
	fail_commit_after_delete: Arc<Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>>,
}
type StoredSecrets = BTreeMap<String, BTreeMap<String, (SecretString, &'static str)>>;
#[async_trait]
impl Store for FakeStore {
	fn resource(&self, id: Uuid) -> String {
		format!("fake/{id}")
	}
	async fn create(&self, _tenant: &str, id: Uuid) -> Result<()> {
		self.state
			.lock()
			.await
			.insert(self.resource(id), BTreeMap::new());
		Ok(())
	}
	async fn add_version(
		&self,
		_tenant: &str,
		resource: &str,
		key: &SecretString,
	) -> Result<String> {
		let mut state = self.state.lock().await;
		let versions = state.get_mut(resource).unwrap();
		let name = format!("{resource}/versions/{}", versions.len() + 1);
		versions.insert(name.clone(), (key.clone(), "enabled"));
		Ok(name)
	}
	async fn versions(&self, resource: &str) -> Result<Vec<String>> {
		self.version_lists.lock().await.push(resource.into());
		Ok(self
			.state
			.lock()
			.await
			.get(resource)
			.map(|v| v.keys().cloned().collect())
			.unwrap_or_default())
	}
	async fn disable(&self, version: &str) -> Result<()> {
		if let Some(error) = self.disable_failure.lock().await.as_ref() {
			return Err(Error::External(error.clone()));
		}
		let mut state = self.state.lock().await;
		for versions in state.values_mut() {
			if let Some(v) = versions.get_mut(version) {
				v.1 = "disabled";
				return Ok(());
			}
		}
		panic!("unknown version")
	}
	async fn destroy(&self, version: &str) -> Result<()> {
		if self.destroy_failure.lock().await.as_deref() == Some(version) {
			return Err(Error::External("destroy-error-canary".into()));
		}
		let mut state = self.state.lock().await;
		for versions in state.values_mut() {
			if let Some(v) = versions.get_mut(version) {
				v.0 = SecretString::from(String::new());
				v.1 = "destroyed";
				return Ok(());
			}
		}
		panic!("unknown version")
	}
	async fn delete(&self, resource: &str) -> Result<()> {
		if *self.delete_failure.lock().await {
			return Err(Error::External("delete-error-canary".into()));
		}
		self.state.lock().await.remove(resource);
		if let Some(flag) = self.fail_commit_after_delete.lock().await.as_ref() {
			flag.store(true, std::sync::atomic::Ordering::SeqCst);
		}
		Ok(())
	}
}
struct Validator;
#[async_trait]
impl KeyValidator for Validator {
	async fn validate(&self, _: Provider, key: &SecretString) -> Result<Validation> {
		if key.expose_secret() == "invalid-key" {
			return Err(Error::Invalid("invalid Provider Credential".into()));
		}
		Ok(Validation {
			warnings: vec!["No spending limit is configured".into()],
		})
	}
}
fn service() -> (Service, Repo, FakeStore) {
	let repo = Repo::default();
	let store = FakeStore::default();
	(
		Service {
			repository: Arc::new(repo.clone()),
			store: Arc::new(store.clone()),
			validator: Arc::new(Validator),
			fingerprint_key: "test-fingerprint-root-key".into(),
			max_per_tenant: 2,
		},
		repo,
		store,
	)
}
async fn create(s: &Service, tenant: &str) -> Validated {
	s.create(
		tenant,
		Uuid::now_v7(),
		Provider::Openrouter,
		"valid-provider-key-1234".into(),
		"actor",
	)
	.await
	.unwrap()
}
#[tokio::test]
async fn lifecycle_rotates_disables_revokes_irreversibly_and_keeps_tombstone() {
	let (s, repo, store) = service();
	let first = create(&s, "alpha").await;
	let id = first.provider_credential.id;
	assert_eq!(first.warnings.len(), 1);
	assert_eq!(first.provider_credential.last4, "1234");
	let rotated = s
		.rotate("alpha", id, 2, "replacement-key-5678".into(), "actor")
		.await
		.unwrap();
	assert_eq!(rotated.provider_credential.revision, 3);
	let state = store.state.lock().await;
	let versions = state.get(&store.resource(id)).unwrap();
	assert_eq!(versions.len(), 2);
	assert_eq!(versions[&format!("fake/{id}/versions/1")].1, "disabled");
	assert_eq!(versions[&format!("fake/{id}/versions/2")].1, "enabled");
	drop(state);
	assert!(matches!(
		s.rotate("alpha", id, 2, "replacement-key-9999".into(), "actor")
			.await,
		Err(Error::Domain(aidash_domain::Error::Conflict(_)))
	));
	let revoked = s.revoke("alpha", id, 3, "actor").await.unwrap();
	assert_eq!(revoked.state, State::Revoked);
	assert!(
		s.rotate("alpha", id, 4, "replacement-key-9999".into(), "actor")
			.await
			.is_err()
	);
	let deleted = s.delete("alpha", id, 4, "actor").await.unwrap();
	assert_eq!(deleted.state, State::Deleted);
	assert!(store.state.lock().await.is_empty());
	assert!(repo.0.lock().await.rows.contains_key(&id));
}
#[tokio::test]
async fn invalid_key_is_never_stored_and_metadata_never_serializes_secret_references() {
	let (s, repo, store) = service();
	let id = Uuid::now_v7();
	assert!(
		s.create(
			"alpha",
			id,
			Provider::Openrouter,
			"invalid-key".into(),
			"actor"
		)
		.await
		.is_err()
	);
	assert!(store.state.lock().await.is_empty());
	assert_eq!(repo.0.lock().await.rows[&id].state, State::Deleted);
	let v = create(&s, "alpha").await;
	let json = serde_json::to_string(&v).unwrap();
	assert!(!json.contains("secret_resource"));
	assert!(!json.contains("pinned_version"));
	assert!(!json.contains("valid-provider-key"));
}
#[tokio::test]
async fn tenant_fingerprints_and_quota_are_isolated_and_bindings_require_active_local_rows() {
	let (s, _, _) = service();
	let a = create(&s, "alpha").await;
	let b = create(&s, "beta").await;
	assert_ne!(
		a.provider_credential.fingerprint,
		b.provider_credential.fingerprint
	);
	assert_eq!(a.provider_credential.fingerprint.len(), 16);
	assert!(
		s.bind(
			"alpha",
			Provider::Openrouter,
			b.provider_credential.id,
			0,
			"actor"
		)
		.await
		.is_err()
	);
	let binding = s
		.bind(
			"alpha",
			Provider::Openrouter,
			a.provider_credential.id,
			0,
			"actor",
		)
		.await
		.unwrap();
	assert_eq!(binding.revision, 1);
	assert!(
		s.bind(
			"alpha",
			Provider::Openrouter,
			a.provider_credential.id,
			0,
			"actor"
		)
		.await
		.is_err()
	);
	assert!(matches!(
		s.delete("alpha", a.provider_credential.id, 2, "actor")
			.await,
		Err(Error::Conflict(_))
	));
	s.revoke("alpha", a.provider_credential.id, 2, "actor")
		.await
		.unwrap();
	assert!(
		s.bind(
			"alpha",
			Provider::Openrouter,
			a.provider_credential.id,
			1,
			"actor"
		)
		.await
		.is_err()
	);
	create(&s, "alpha").await;
	assert!(matches!(
		s.create(
			"alpha",
			Uuid::now_v7(),
			Provider::Openrouter,
			"valid-provider-key".into(),
			"actor"
		)
		.await,
		Err(Error::Conflict(_))
	));
}
#[tokio::test]
async fn pending_reconciliation_uses_rows_and_cleans_only_expired_pending_resources() {
	let (s, repo, store) = service();
	let active = create(&s, "alpha").await;
	let active_id = active.provider_credential.id;
	let mut pending = repo.0.lock().await.rows[&active_id].clone();
	pending.id = Uuid::now_v7();
	pending.secret_resource = store.resource(pending.id);
	pending.state = State::Pending;
	pending.created_at = Utc::now() - Duration::minutes(6);
	let id = pending.id;
	repo.0.lock().await.rows.insert(id, pending);
	store.create("alpha", id).await.unwrap();
	assert_eq!(s.reconcile().await.unwrap(), 1);
	assert_eq!(repo.0.lock().await.rows[&id].state, State::Deleted);
	assert!(
		store
			.state
			.lock()
			.await
			.contains_key(&store.resource(active_id))
	);
}

#[tokio::test]
async fn access_uses_the_admitted_tenant_id_and_never_falls_back_to_environment() {
	use crate::provider_access::{
		Context, EnvironmentAccess, ProviderAccess, Source, TenantAccess,
	};
	struct Env;
	impl crate::ports::Credentials for Env {
		fn resolve(&self, _: &str) -> Result<String> {
			panic!("Tenant access must never resolve environment Key Material")
		}
	}
	let (s, repo, _) = service();
	let a = create(&s, "alpha").await;
	let b = create(&s, "beta").await;
	let access = TenantAccess {
		environment: EnvironmentAccess {
			credentials: Arc::new(Env),
		},
		repository: Arc::new(repo),
		reader: None,
	};
	let source = Source::Tenant {
		provider: "openrouter".into(),
	};
	let mut context = Context {
		tenant: "beta".into(),
		run: Some(Uuid::now_v7()),
		maintenance: None,
		provider_credential_id: Some(a.provider_credential.id),
	};
	assert!(matches!(
		access
			.resolve(&context, Provider::Openrouter.base_url(), &source)
			.await,
		Err(Error::NotFound(_))
	));
	context.tenant = "alpha".into();
	let error = access
		.resolve(&context, Provider::Openrouter.base_url(), &source)
		.await
		.unwrap_err();
	assert_eq!(error.to_string(), "credential broker not configured");
	s.bind(
		"alpha",
		Provider::Openrouter,
		a.provider_credential.id,
		0,
		"actor",
	)
	.await
	.unwrap();
	let other = create(&s, "alpha").await;
	s.bind(
		"alpha",
		Provider::Openrouter,
		other.provider_credential.id,
		1,
		"actor",
	)
	.await
	.unwrap();
	s.revoke("alpha", a.provider_credential.id, 2, "actor")
		.await
		.unwrap();
	assert!(
		access
			.resolve(&context, Provider::Openrouter.base_url(), &source)
			.await
			.unwrap_err()
			.to_string()
			.contains("not active")
	);
	context.tenant = "beta".into();
	context.provider_credential_id = Some(b.provider_credential.id);
	s.rotate(
		"beta",
		b.provider_credential.id,
		2,
		"rotated-provider-key-4321".into(),
		"actor",
	)
	.await
	.unwrap();
	assert_eq!(
		access
			.resolve(&context, Provider::Openrouter.base_url(), &source)
			.await
			.unwrap_err()
			.to_string(),
		"credential broker not configured"
	);
}

#[tokio::test]
async fn unbind_preserves_revision_and_allows_deleting_the_last_credential() {
	let (s, _, _) = service();
	let row = create(&s, "alpha").await.provider_credential;
	s.bind("alpha", Provider::Openrouter, row.id, 0, "actor")
		.await
		.unwrap();
	assert!(matches!(
		s.bind("alpha", Provider::Openrouter, None, 0, "actor")
			.await,
		Err(Error::Conflict(_))
	));
	let unbound = s
		.bind("alpha", Provider::Openrouter, None, 1, "actor")
		.await
		.unwrap();
	assert_eq!(unbound.revision, 2);
	assert_eq!(unbound.provider_credential_id, None);
	assert_eq!(
		s.delete("alpha", row.id, row.revision, "actor")
			.await
			.unwrap()
			.state,
		State::Deleted
	);
}
#[tokio::test]
async fn reconciler_disables_unpinned_versions_left_by_a_rotation_crash() {
	let (s, repo, store) = service();
	let row = create(&s, "alpha").await.provider_credential;
	let metadata = repo.0.lock().await.rows[&row.id].clone();
	let abandoned = store
		.add_version(
			"alpha",
			&metadata.secret_resource,
			&SecretString::from("uncommitted-key-1234"),
		)
		.await
		.unwrap();
	s.reconcile().await.unwrap();
	let states = store.state.lock().await;
	assert_eq!(states[&metadata.secret_resource][&abandoned].1, "disabled");
	assert_eq!(
		states[&metadata.secret_resource][metadata.pinned_version.as_ref().unwrap()].1,
		"enabled"
	);
}

#[async_trait]
impl crate::provider_access::KeyMaterialReader for FakeStore {
	async fn read(&self, _: &str, resource: &str, version: &str) -> Result<SecretString> {
		let state = self.state.lock().await;
		let (key, status) = state.get(resource).and_then(|v| v.get(version)).unwrap();
		if *status != "enabled" {
			return Err(Error::Invalid("disabled version".into()));
		}
		Ok(key.clone())
	}
}
#[tokio::test]
async fn reader_resolves_current_pin_after_metadata_checks() {
	use crate::provider_access::{
		Context, EnvironmentAccess, ProviderAccess, Source, TenantAccess,
	};
	struct Env;
	impl crate::ports::Credentials for Env {
		fn resolve(&self, _: &str) -> Result<String> {
			panic!("no environment fallback")
		}
	}
	let (s, repo, store) = service();
	let row = create(&s, "alpha").await.provider_credential;
	let access = TenantAccess {
		environment: EnvironmentAccess {
			credentials: Arc::new(Env),
		},
		repository: Arc::new(repo),
		reader: Some(Arc::new(store)),
	};
	let mut context = Context {
		tenant: "alpha".into(),
		run: Some(Uuid::now_v7()),
		provider_credential_id: Some(row.id),
		..Default::default()
	};
	let source = Source::Tenant {
		provider: "openrouter".into(),
	};
	let endpoint = Provider::Openrouter.base_url();
	assert_eq!(
		access
			.resolve(&context, endpoint, &source)
			.await
			.unwrap()
			.bearer
			.expose_secret(),
		"valid-provider-key-1234"
	);
	s.rotate(
		"alpha",
		row.id,
		2,
		"replacement-provider-key-5678".into(),
		"actor",
	)
	.await
	.unwrap();
	assert_eq!(
		access
			.resolve(&context, endpoint, &source)
			.await
			.unwrap()
			.bearer
			.expose_secret(),
		"replacement-provider-key-5678"
	);
	assert!(
		access
			.resolve(&context, "https://example.com", &source)
			.await
			.is_err()
	);
	context.tenant = "beta".into();
	assert!(access.resolve(&context, endpoint, &source).await.is_err());
	context.tenant = "alpha".into();
	s.revoke("alpha", row.id, 3, "actor").await.unwrap();
	assert!(access.resolve(&context, endpoint, &source).await.is_err());
}

#[tokio::test]
async fn committed_rotation_returns_metadata_and_reconciles_failed_old_version_cleanup() {
	let (s, repo, store) = service();
	let first = create(&s, "alpha").await.provider_credential;
	let original = repo.0.lock().await.rows[&first.id].clone();
	*store.disable_failure.lock().await = Some("rotation-cleanup-error-canary".into());
	let rotated = s
		.rotate(
			"alpha",
			first.id,
			first.revision,
			"replacement-key-5678".into(),
			"actor",
		)
		.await
		.unwrap();
	assert_eq!(rotated.provider_credential.revision, first.revision + 1);
	assert_eq!(rotated.provider_credential.last4, "5678");
	assert_eq!(rotated.provider_credential.state, State::Active);
	assert_eq!(
		rotated.warnings,
		[
			"No spending limit is configured",
			"Provider Credential rotation committed; previous version cleanup is pending",
		]
	);
	let serialized = serde_json::to_string(&rotated).unwrap();
	assert!(!serialized.contains("rotation-cleanup-error-canary"));
	assert!(!serialized.contains("replacement-key"));
	let committed = repo.0.lock().await.rows[&first.id].clone();
	assert_eq!(committed.revision, rotated.provider_credential.revision);
	assert_ne!(committed.pinned_version, original.pinned_version);
	let old = original.pinned_version.unwrap();
	let new = committed.pinned_version.unwrap();
	assert!(s.reconcile().await.is_err());
	{
		let versions = store.state.lock().await;
		assert_eq!(versions[&committed.secret_resource][&old].1, "enabled");
		assert_eq!(versions[&committed.secret_resource][&new].1, "enabled");
	}
	*store.disable_failure.lock().await = None;
	s.reconcile().await.unwrap();
	let versions = store.state.lock().await;
	assert_eq!(versions[&committed.secret_resource][&old].1, "disabled");
	assert_eq!(versions[&committed.secret_resource][&new].1, "enabled");
	assert_eq!(
		repo.0.lock().await.rows[&first.id].revision,
		committed.revision
	);
}

#[tokio::test]
async fn reconciliation_pages_bound_work_and_reach_later_tenants_after_failures() {
	let (mut s, repo, store) = service();
	s.max_per_tenant = 100;
	let mut rows = Vec::new();
	for tenant in ["tenant-a", "tenant-b"] {
		for _ in 0..16 {
			rows.push(create(&s, tenant).await.provider_credential.id);
		}
	}
	rows.sort();
	// Leave an unpinned version on the first candidate, and simulate an outage.
	let first = repo.0.lock().await.rows[&rows[0]].clone();
	store
		.add_version(
			&first.tenant,
			&first.secret_resource,
			&"uncommitted-version-key".into(),
		)
		.await
		.unwrap();
	*store.disable_failure.lock().await = Some("external outage".into());
	let first_page = s.reconcile_page(None).await.unwrap();
	assert_eq!(first_page.cleaned, 0);
	assert_eq!(first_page.failed, 1);
	assert_eq!(first_page.next, Some(rows[24]));
	assert_eq!(store.version_lists.lock().await.len(), 25);
	let second_page = s.reconcile_page(first_page.next).await.unwrap();
	assert_eq!(second_page.failed, 0);
	assert_eq!(second_page.next, None);
	let visited = store.version_lists.lock().await.clone();
	assert_eq!(visited.len(), 32);
	assert_eq!(
		visited
			.iter()
			.collect::<std::collections::BTreeSet<_>>()
			.len(),
		32
	);
	*store.disable_failure.lock().await = None;
	assert_eq!(s.reconcile_page(None).await.unwrap().failed, 0);
}

#[tokio::test]
async fn revocation_commit_failure_has_no_external_effect_and_committed_cleanup_is_recoverable() {
	use crate::provider_access::{
		Context, EnvironmentAccess, ProviderAccess, Source, TenantAccess,
	};
	struct Env;
	impl crate::ports::Credentials for Env {
		fn resolve(&self, _: &str) -> Result<String> {
			panic!("revoked Tenant access cannot fall back to environment")
		}
	}
	let (s, repo, store) = service();
	let first = create(&s, "alpha").await.provider_credential;
	let original = repo.0.lock().await.rows[&first.id].clone();
	store
		.add_version(
			&original.tenant,
			&original.secret_resource,
			&"unpinned-key-5678".into(),
		)
		.await
		.unwrap();
	repo.0.lock().await.commit_failure = true;
	assert!(
		s.revoke("alpha", first.id, first.revision, "actor")
			.await
			.is_err()
	);
	assert!(store.version_lists.lock().await.is_empty());
	{
		let data = repo.0.lock().await;
		assert_eq!(data.rows[&first.id].state, State::Active);
		assert_eq!(data.rows[&first.id].revision, first.revision);
		let versions = store.state.lock().await;
		assert!(
			versions[&original.secret_resource]
				.values()
				.all(|v| v.1 == "enabled")
		);
	}
	repo.0.lock().await.commit_failure = false;
	*store.disable_failure.lock().await = Some("cleanup-error-canary".into());
	let revoked = s
		.revoke("alpha", first.id, first.revision, "actor")
		.await
		.unwrap();
	assert_eq!(revoked.state, State::Revoked);
	assert_eq!(revoked.revision, first.revision + 1);
	assert_eq!(repo.0.lock().await.rows[&first.id].state, State::Revoked);
	assert!(
		!serde_json::to_string(&revoked)
			.unwrap()
			.contains("cleanup-error-canary")
	);
	let access = TenantAccess {
		environment: EnvironmentAccess {
			credentials: Arc::new(Env),
		},
		repository: Arc::new(repo.clone()),
		reader: Some(Arc::new(store.clone())),
	};
	let error = access
		.resolve(
			&Context {
				tenant: "alpha".into(),
				run: Some(Uuid::now_v7()),
				provider_credential_id: Some(first.id),
				maintenance: None,
			},
			Provider::Openrouter.base_url(),
			&Source::Tenant {
				provider: "openrouter".into(),
			},
		)
		.await
		.unwrap_err();
	assert!(error.to_string().contains("not active"));
	assert_eq!(s.reconcile_page(None).await.unwrap().failed, 1);
	*store.disable_failure.lock().await = None;
	assert_eq!(s.reconcile_page(None).await.unwrap().failed, 0);
	assert!(
		store.state.lock().await[&original.secret_resource]
			.values()
			.all(|v| v.1 == "disabled")
	);
	assert_eq!(
		repo.0.lock().await.rows[&first.id].revision,
		revoked.revision
	);
}

#[tokio::test]
async fn deletion_commit_failure_preserves_metadata_and_key_material() {
	let (s, repo, store) = service();
	let first = create(&s, "alpha").await.provider_credential;
	let original = repo.0.lock().await.rows[&first.id].clone();
	repo.0.lock().await.commit_failure = true;
	assert!(
		s.delete("alpha", first.id, first.revision, "actor")
			.await
			.is_err()
	);
	assert!(store.version_lists.lock().await.is_empty());
	let data = repo.0.lock().await;
	assert_eq!(data.rows[&first.id].state, State::Active);
	assert_eq!(data.rows[&first.id].revision, first.revision);
	assert_eq!(data.rows[&first.id].pinned_version, original.pinned_version);
	assert!(
		data.events
			.iter()
			.all(|(_, event, _)| event != "provider_credential.deleted")
	);
	assert!(
		store.state.lock().await[&original.secret_resource]
			.values()
			.all(|v| v.1 == "enabled")
	);
}

#[tokio::test]
async fn deleted_intent_recovers_partial_destruction_secret_deletion_and_final_commit_failures() {
	use std::sync::atomic::Ordering;
	for failure in ["destroy", "delete", "commit"] {
		let (s, repo, store) = service();
		let first = create(&s, "alpha").await.provider_credential;
		let original = repo.0.lock().await.rows[&first.id].clone();
		let extra = store
			.add_version(
				&original.tenant,
				&original.secret_resource,
				&"extra-key-5678".into(),
			)
			.await
			.unwrap();
		match failure {
			"destroy" => *store.destroy_failure.lock().await = Some(extra.clone()),
			"delete" => *store.delete_failure.lock().await = true,
			"commit" => *store.fail_commit_after_delete.lock().await = Some(repo.1.clone()),
			_ => unreachable!(),
		}
		let deleted = s
			.delete("alpha", first.id, first.revision, "actor")
			.await
			.unwrap();
		assert_eq!(deleted.state, State::Deleted);
		assert_eq!(deleted.revision, first.revision + 1);
		assert!(
			!serde_json::to_string(&deleted)
				.unwrap()
				.contains("error-canary")
		);
		{
			let data = repo.0.lock().await;
			let row = &data.rows[&first.id];
			assert_eq!(row.state, State::Deleted);
			assert_eq!(row.pinned_version, original.pinned_version);
			assert!(row.require_active().is_err());
			assert_eq!(
				data.events
					.iter()
					.filter(|(_, e, _)| e == "provider_credential.deleted")
					.count(),
				1
			);
			assert_eq!(
				data.events
					.iter()
					.filter(|(_, e, _)| e == "provider_credential.cleanup_completed")
					.count(),
				0
			);
		}
		if failure == "destroy" {
			let secrets = store.state.lock().await;
			assert_eq!(
				secrets[&original.secret_resource][original.pinned_version.as_ref().unwrap()].1,
				"destroyed"
			);
			assert_eq!(secrets[&original.secret_resource][&extra].1, "enabled");
		} else if failure == "delete" {
			assert!(
				store.state.lock().await[&original.secret_resource]
					.values()
					.all(|v| v.1 == "destroyed")
			);
		} else {
			assert!(
				!store
					.state
					.lock()
					.await
					.contains_key(&original.secret_resource)
			);
		}
		*store.destroy_failure.lock().await = None;
		*store.delete_failure.lock().await = false;
		*store.fail_commit_after_delete.lock().await = None;
		repo.1.store(false, Ordering::SeqCst);
		// A restarted service has only persisted metadata as its cleanup inventory.
		let restarted = Service {
			repository: Arc::new(repo.clone()),
			store: Arc::new(store.clone()),
			validator: Arc::new(Validator),
			fingerprint_key: "test-fingerprint-root-key".into(),
			max_per_tenant: 2,
		};
		let page = restarted.reconcile_page(None).await.unwrap();
		assert_eq!(page.cleaned, 1);
		assert_eq!(page.failed, 0);
		assert!(
			!store
				.state
				.lock()
				.await
				.contains_key(&original.secret_resource)
		);
		let data = repo.0.lock().await;
		assert_eq!(data.rows[&first.id].state, State::Deleted);
		assert_eq!(data.rows[&first.id].revision, deleted.revision);
		assert_eq!(data.rows[&first.id].pinned_version, None);
		assert_eq!(
			data.events
				.iter()
				.filter(|(_, e, _)| e == "provider_credential.deleted")
				.count(),
			1
		);
		assert_eq!(
			data.events
				.iter()
				.filter(|(_, e, _)| e == "provider_credential.cleanup_completed")
				.count(),
			1
		);
		drop(data);
		assert!(
			repo.reconciliation_candidates(None, 25)
				.await
				.unwrap()
				.is_empty()
		);
		assert_eq!(restarted.reconcile_page(None).await.unwrap().cleaned, 0);
	}
}
