use super::*;
use std::collections::BTreeMap;
use tokio::sync::{Mutex, OwnedMutexGuard};
#[derive(Clone, Default)]
struct Data {
	rows: BTreeMap<Uuid, ProviderCredential>,
	bindings: BTreeMap<String, Binding>,
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
	async fn pending(&self) -> Result<Vec<ProviderCredential>> {
		Ok(self
			.0
			.lock()
			.await
			.rows
			.values()
			.filter(|r| r.state == State::Pending)
			.cloned()
			.collect())
	}
	async fn active(&self) -> Result<Vec<ProviderCredential>> {
		Ok(self
			.0
			.lock()
			.await
			.rows
			.values()
			.filter(|r| r.state == State::Active)
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
	async fn save(&mut self, row: &ProviderCredential, _: &str, _: &str) -> Result<()> {
		assert_eq!(row.tenant, self.tenant);
		self.data.rows.insert(row.id, row.clone());
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
		*self.guard = self.data.clone();
		Ok(())
	}
}
#[derive(Clone, Default)]
struct FakeStore {
	state: Arc<Mutex<StoredSecrets>>,
}
type StoredSecrets = BTreeMap<String, BTreeMap<String, (SecretString, &'static str)>>;
#[async_trait]
impl Store for FakeStore {
	fn resource(&self, id: Uuid) -> String {
		format!("fake/{id}")
	}
	async fn create(&self, id: Uuid) -> Result<()> {
		self.state
			.lock()
			.await
			.insert(self.resource(id), BTreeMap::new());
		Ok(())
	}
	async fn add_version(&self, resource: &str, key: &SecretString) -> Result<String> {
		let mut state = self.state.lock().await;
		let versions = state.get_mut(resource).unwrap();
		let name = format!("{resource}/versions/{}", versions.len() + 1);
		versions.insert(name.clone(), (key.clone(), "enabled"));
		Ok(name)
	}
	async fn versions(&self, resource: &str) -> Result<Vec<String>> {
		Ok(self
			.state
			.lock()
			.await
			.get(resource)
			.map(|v| v.keys().cloned().collect())
			.unwrap_or_default())
	}
	async fn disable(&self, version: &str) -> Result<()> {
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
		self.state.lock().await.remove(resource);
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
	store.create(id).await.unwrap();
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
