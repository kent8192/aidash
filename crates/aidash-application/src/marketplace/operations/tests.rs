use super::*;
use crate::marketplace::installations::{
	self,
	tests::{Fixture, input, published, validation},
};
use crate::ports::{marketplace::StagingScope, registry::PrivateKnowledgeRead};
use aidash_domain::marketplace::Version;
use aidash_domain::{
	identity::Principal,
	marketplace::{Audience, ConsentEdge, Revision},
	registry::Entry,
};
use async_trait::async_trait;
use rstest::rstest;
use std::collections::BTreeMap;
use uuid::Uuid;

pub(super) struct DeploymentAdministratorFixture {
	pub(super) inner: Fixture,
	pub(super) principal: Principal,
	pub(super) approving: Option<String>,
	pub(super) approved: BTreeSet<(String, String)>,
	requests: BTreeMap<String, Value>,
	pub(super) events: Vec<(String, Value)>,
	pub(super) approvals: Vec<(String, EntityRef, i64, bool)>,
}
impl DeploymentAdministratorFixture {
	pub(super) fn new() -> Self {
		Self {
			inner: published(),
			principal: Principal::DeploymentAdministrator,
			approving: Some("owner".into()),
			approved: BTreeSet::new(),
			requests: BTreeMap::new(),
			events: vec![],
			approvals: vec![],
		}
	}
	pub(super) async fn installed() -> (Self, String) {
		let mut scope = Self::new();
		let request = input(&scope.inner);
		let installation =
			installations::install(&mut scope.inner, &validation(), "package", &request, "node")
				.await
				.unwrap();
		scope.inner.calls.clear();
		(scope, installation.installation.id)
	}
}
#[async_trait]
impl PrivateKnowledgeRead for DeploymentAdministratorFixture {
	async fn documents(&mut self, entry: &Entry) -> Result<Option<Value>> {
		self.inner.documents(entry).await
	}
}
#[async_trait]
impl StagingScope for DeploymentAdministratorFixture {
	fn actor(&self) -> &str {
		"deployment-administrator-adoption"
	}
	fn allocate_entry_id(&mut self) -> Uuid {
		self.inner.allocate_entry_id()
	}
	async fn persist_revision(
		&mut self,
		install: &Installation,
		revision: &Revision,
	) -> Result<()> {
		self.inner.persist_revision(install, revision).await
	}
	async fn insert_documents(&mut self, entry: &Entry, documents: Value) -> Result<()> {
		self.inner.insert_documents(entry, documents).await
	}
	async fn save_provenance(
		&mut self,
		entry: &Entry,
		edges: &BTreeSet<ConsentEdge>,
	) -> Result<()> {
		self.inner.save_provenance(entry, edges).await
	}
	async fn copy_provenance(
		&mut self,
		entry: &Entry,
		tenant: &str,
	) -> Result<BTreeSet<ConsentEdge>> {
		self.inner.copy_provenance(entry, tenant).await
	}
	async fn save_copy_provenance(
		&mut self,
		entry: &Entry,
		tenant: &str,
		edges: &BTreeSet<ConsentEdge>,
	) -> Result<()> {
		self.inner.save_copy_provenance(entry, tenant, edges).await
	}
	async fn installed_event(&mut self, payload: Value) -> Result<()> {
		self.events.push(("marketplace.installed".into(), payload));
		self.inner.installed_event(json!({})).await
	}
}
#[async_trait]
impl DeploymentAdministratorScope for DeploymentAdministratorFixture {
	fn principal(&self) -> &Principal {
		&self.principal
	}
	fn approving_tenant(&self) -> Option<&str> {
		self.approving.as_deref()
	}
	async fn load_tenant(&mut self, _: &str) -> Result<()> {
		self.inner.record("load_tenant")
	}
	async fn distribution_lock(&mut self, exclusive: bool) -> Result<()> {
		self.inner.record(if exclusive {
			"exclusive_lock"
		} else {
			"shared_lock"
		})
	}
	async fn enable_writer(&mut self) -> Result<()> {
		self.inner.record("enable_writer")
	}
	async fn installation(&mut self, id: &str) -> Result<Option<Installation>> {
		Ok(self.inner.installations.get(id).cloned())
	}
	async fn revision(&mut self, id: &str, revision: i64) -> Result<Revision> {
		self.inner
			.revisions
			.get(&format!("{id}:{revision}"))
			.cloned()
			.ok_or(Error::Forbidden)
	}
	async fn approved(&mut self, _: &str, reference: &EntityRef) -> Result<bool> {
		Ok(self
			.approved
			.contains(&(reference.id.clone(), reference.version.clone())))
	}
	async fn set_approval(
		&mut self,
		tenant: &str,
		reference: &EntityRef,
		expected: i64,
		enabled: bool,
		_restriction: aidash_domain::identity::catalog::RestrictionChange,
	) -> Result<()> {
		self.inner.record("set_approval")?;
		self.approvals
			.push((tenant.into(), reference.clone(), expected, enabled));
		if enabled {
			self.approved
				.insert((reference.id.clone(), reference.version.clone()));
		} else {
			self.approved
				.remove(&(reference.id.clone(), reference.version.clone()));
		}
		Ok(())
	}
	async fn catalog_revision(&mut self, _: &str, reference: &EntityRef) -> Result<i64> {
		Ok(
			if self
				.approved
				.contains(&(reference.id.clone(), reference.version.clone()))
			{
				1
			} else {
				0
			},
		)
	}
	async fn save_installation(&mut self, installation: &Installation) -> Result<()> {
		self.inner.record("save_installation")?;
		self.inner
			.installations
			.insert(installation.id.clone(), installation.clone());
		Ok(())
	}
	async fn event(&mut self, kind: &str, payload: Value) -> Result<()> {
		self.inner.record(&format!("event:{kind}"))?;
		self.events.push((kind.into(), payload));
		Ok(())
	}
	async fn adoption_replay(&mut self, key: &str) -> Result<Option<Value>> {
		Ok(self.requests.get(key).cloned())
	}
	async fn remember_adoption(&mut self, key: &str, record: Value) -> Result<()> {
		self.inner.record("remember_adoption")?;
		self.requests.insert(key.into(), record);
		Ok(())
	}
	async fn raw_definition(&mut self, reference: &EntityRef) -> Result<Entry> {
		self.inner
			.entries
			.get(&reference.id)
			.cloned()
			.ok_or(Error::Forbidden)
	}
	async fn provenance(&mut self, entry: &Entry, _: &str) -> Result<BTreeSet<ConsentEdge>> {
		Ok(self
			.inner
			.provenance
			.get(&entry.id)
			.cloned()
			.unwrap_or_default())
	}
	async fn version(&mut self, key: &str) -> Result<Option<Version>> {
		Ok(self.inner.versions.get(key).cloned())
	}
	async fn insert_version(&mut self, version: &Version, audience: &Audience) -> Result<()> {
		self.inner.record("insert_version")?;
		self.inner
			.versions
			.insert(version.key.clone(), version.clone());
		self.inner
			.audiences
			.insert(version.key.clone(), audience.clone());
		Ok(())
	}
	async fn revision_documents(
		&mut self,
		tenant: &str,
		offset: usize,
		limit: u64,
	) -> Result<Vec<(Value, Value)>> {
		self.inner.record("revision_documents")?;
		self.inner
			.revisions
			.values()
			.filter(|r| r.tenant == tenant)
			.skip(offset)
			.take(limit as usize)
			.map(|r| {
				Ok((
					serde_json::to_value(&self.inner.installations[&r.installation])?,
					serde_json::to_value(r)?,
				))
			})
			.collect()
	}
}
fn activation_input(enabled: bool) -> ActivationCommand {
	ActivationCommand {
		tenant: "owner".into(),
		revision: 1,
		expected_activation_revision: 0,
		expected_catalog_revision: 5,
		enabled,
	}
}

#[rstest]
#[case("activate")]
#[case("administration")]
#[tokio::test]
async fn activation_needs_the_approving_tenant_and_administration_a_deployment_administrator(
	#[case] operation: &str,
) {
	// Arrange: a subject with no `catalog.approve` decision for this Tenant.
	let mut scope = DeploymentAdministratorFixture::new();
	scope.principal = Principal::Subject {
		tenant: "owner".into(),
		subject: "subject".into(),
	};
	scope.approving = None;
	// Act
	let result = match operation {
		"activate" => activate(&mut scope, "installation", &activation_input(true))
			.await
			.map(|_| ()),
		"administration" => administration(
			&mut scope,
			&AdministrationQuery {
				tenant: "owner".into(),
				offset: 0,
				limit: 1,
			},
		)
		.await
		.map(|_| ()),
		_ => panic!("unknown fixture operation"),
	};
	// Assert
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(scope.inner.calls, Vec::<String>::new());
	assert_eq!(scope.events.len(), 0);
}
#[rstest]
#[case(true, Some(1))]
#[case(false, Some(7))]
#[tokio::test]
async fn activation_and_revocation_preserve_pointer_semantics_and_effect_order(
	#[case] enabled: bool,
	#[case] expected: Option<i64>,
) {
	// Arrange
	let (mut scope, id) = DeploymentAdministratorFixture::installed().await;
	scope
		.inner
		.installations
		.get_mut(&id)
		.unwrap()
		.active_revision = Some(7);
	// Act
	let result = activate(&mut scope, &id, &activation_input(enabled))
		.await
		.unwrap();
	// Assert
	assert_eq!(result.active_revision, expected);
	assert_eq!(result.activation_revision, 1);
	assert_eq!(scope.approvals[0].2, 5);
	assert_eq!(scope.approvals[0].3, enabled);
	assert_eq!(
		scope.inner.calls,
		[
			"load_tenant",
			"exclusive_lock",
			"enable_writer",
			"set_approval",
			"save_installation",
			"event:marketplace.activation_changed",
		]
	);
	assert_eq!(scope.events[0].1["actor"], "deployment_administrator");
}
#[rstest]
#[tokio::test]
async fn revoked_dependency_prevents_catalog_and_pointer_changes() {
	let (mut scope, id) = DeploymentAdministratorFixture::installed().await;
	scope
		.inner
		.revisions
		.get_mut(&format!("{id}:1"))
		.unwrap()
		.dependencies
		.push(EntityRef {
			id: "revoked".into(),
			version: "1.0.0".into(),
		});
	let result = activate(&mut scope, &id, &activation_input(true)).await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(scope.approvals.len(), 0);
	assert_eq!(scope.inner.installations[&id].active_revision, None);
	assert_eq!(scope.events.len(), 0);
}
#[rstest]
#[tokio::test]
async fn denied_catalog_write_cannot_advance_the_active_pointer_or_emit_an_event() {
	let (mut scope, id) = DeploymentAdministratorFixture::installed().await;
	scope.inner.denied = Some("set_approval".into());
	let result = activate(&mut scope, &id, &activation_input(true)).await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(scope.inner.installations[&id].activation_revision, 0);
	assert_eq!(scope.events.len(), 0);
}
#[rstest]
#[tokio::test]
async fn administration_preserves_candidate_progress_and_has_no_subject_actions() {
	let (mut scope, id) = DeploymentAdministratorFixture::installed().await;
	let original = scope.inner.revisions[&format!("{id}:1")].clone();
	scope.inner.revisions.insert(
		format!("{id}:2"),
		Revision {
			revision: 2,
			..original
		},
	);
	let first = administration(
		&mut scope,
		&AdministrationQuery {
			tenant: "owner".into(),
			offset: 0,
			limit: 1,
		},
	)
	.await
	.unwrap();
	let second = administration(
		&mut scope,
		&AdministrationQuery {
			tenant: "owner".into(),
			offset: first.next_offset.unwrap(),
			limit: 1,
		},
	)
	.await
	.unwrap();
	assert_eq!(first.entries[0].revision, 1);
	assert_eq!(second.entries[0].revision, 2);
	assert_eq!(first.next_offset, Some(1));
	assert_eq!(second.next_offset, None);
	assert_eq!(first.entries[0].actions, Vec::<String>::new());
}

use serde_json::Value;
