use super::*;
use crate::marketplace::installations::tests::{Fixture, entry, input, published, validation};
use crate::ports::{marketplace::StagingScope, registry::PrivateKnowledgeRead};
use aidash_domain::{
	identity::Principal,
	marketplace::{Audience, ConsentEdge, Revision, SourceGrant},
	registry::{Entry, Package},
};
use async_trait::async_trait;
use rstest::rstest;
use std::collections::BTreeMap;

pub(super) struct OperatorFixture {
	pub(super) inner: Fixture,
	pub(super) principal: Principal,
	state: Compatibility,
	pub(super) approved: BTreeSet<(String, String)>,
	overlays: BTreeMap<String, Entry>,
	records: BTreeMap<String, (String, String)>,
	config: BTreeMap<String, Value>,
	requests: BTreeMap<String, Value>,
	pub(super) events: Vec<(String, Value)>,
	pub(super) approvals: Vec<(String, EntityRef, i64, bool)>,
}
impl OperatorFixture {
	pub(super) fn new() -> Self {
		let mut scope = Self {
			inner: published(),
			principal: Principal::Operator,
			state: Compatibility {
				enabled: true,
				revision: 2,
				contract: 1,
			},
			approved: BTreeSet::new(),
			overlays: BTreeMap::new(),
			records: BTreeMap::new(),
			config: BTreeMap::new(),
			requests: BTreeMap::new(),
			events: vec![],
			approvals: vec![],
		};
		scope.package_record("source");
		scope
	}
	fn package_record(&mut self, id: &str) {
		let package = Package {
			entity: self.inner.entries[id].clone(),
			author: "Fixture".into(),
			permissions: vec![],
			dependencies: vec![],
		};
		let bytes = serde_json::to_string(&package).unwrap();
		let digest = format!("sha256:{:x}", Sha256::digest(bytes.as_bytes()));
		self.records.insert(id.into(), (bytes, digest));
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
impl PrivateKnowledgeRead for OperatorFixture {
	async fn documents(&mut self, entry: &Entry) -> Result<Option<Value>> {
		self.inner.documents(entry).await
	}
}
#[async_trait]
impl StagingScope for OperatorFixture {
	fn actor(&self) -> &str {
		"operator-adoption"
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
impl OperatorScope for OperatorFixture {
	fn principal(&self) -> &Principal {
		&self.principal
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
	async fn compatibility(&mut self) -> Result<Option<Compatibility>> {
		self.inner.record("compatibility")?;
		Ok(Some(self.state.clone()))
	}
	async fn save_compatibility(&mut self, state: &Compatibility) -> Result<()> {
		self.inner.record("save_compatibility")?;
		self.state = state.clone();
		Ok(())
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
	async fn effective_legacy(&mut self, reference: &EntityRef) -> Result<Entry> {
		self.overlays
			.get(&reference.id)
			.or_else(|| self.inner.entries.get(&reference.id))
			.cloned()
			.ok_or(Error::Forbidden)
	}
	async fn package_record(&mut self, reference: &EntityRef) -> Result<Option<(String, String)>> {
		Ok(self.records.get(&reference.id).cloned())
	}
	async fn legacy_config(&mut self, reference: &EntityRef) -> Result<Option<Value>> {
		Ok(self.config.get(&reference.id).cloned())
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
fn adoption() -> AdoptionCommand {
	AdoptionCommand {
		tenant: "owner".into(),
		source: EntityRef {
			id: "source".into(),
			version: "1.0.0".into(),
		},
		idempotency_key: Uuid::nil(),
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
#[case("read")]
#[case("change")]
#[case("activate")]
#[case("adopt")]
#[case("administration")]
#[tokio::test]
async fn every_operator_workflow_denies_subjects_before_persistence(#[case] operation: &str) {
	// Arrange: trusted authentication says this caller is a subject, not an operator.
	let mut scope = OperatorFixture::new();
	scope.principal = Principal::Subject {
		tenant: "owner".into(),
		subject: "subject".into(),
	};
	// Act
	let result = match operation {
		"read" => compatibility(&mut scope).await.map(|_| ()),
		"change" => set_compatibility(
			&mut scope,
			&CompatibilityChange {
				enabled: false,
				expected_revision: 2,
				compatible_instances_confirmed: true,
			},
		)
		.await
		.map(|_| ()),
		"activate" => activate(&mut scope, "installation", &activation_input(true))
			.await
			.map(|_| ()),
		"adopt" => adopt(&mut scope, &validation(), &adoption(), "node")
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
#[tokio::test]
async fn compatibility_enable_requires_confirmation_before_acquiring_distribution_lease() {
	let mut scope = OperatorFixture::new();
	let result = set_compatibility(
		&mut scope,
		&CompatibilityChange {
			enabled: true,
			expected_revision: 2,
			compatible_instances_confirmed: false,
		},
	)
	.await;
	assert!(
		matches!(result,Err(Error::Invalid(message)) if message=="confirm every serving instance and worker supports Marketplace contract 1")
	);
	assert_eq!(scope.inner.calls, Vec::<String>::new());
}
#[rstest]
#[tokio::test]
async fn stale_compatibility_revision_has_no_state_or_event_effect() {
	let mut scope = OperatorFixture::new();
	let result = set_compatibility(
		&mut scope,
		&CompatibilityChange {
			enabled: false,
			expected_revision: 1,
			compatible_instances_confirmed: false,
		},
	)
	.await;
	assert!(matches!(result, Err(Error::Conflict(_))));
	assert_eq!(scope.state.revision, 2);
	assert!(scope.state.enabled);
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
	let (mut scope, id) = OperatorFixture::installed().await;
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
		if enabled {
			vec![
				"load_tenant",
				"exclusive_lock",
				"compatibility",
				"enable_writer",
				"set_approval",
				"save_installation",
				"event:marketplace.activation_changed",
			]
		} else {
			vec![
				"load_tenant",
				"exclusive_lock",
				"enable_writer",
				"set_approval",
				"save_installation",
				"event:marketplace.activation_changed",
			]
		}
	);
	assert_eq!(scope.events[0].1["actor"], "operator");
}
#[rstest]
#[tokio::test]
async fn revoked_dependency_prevents_catalog_and_pointer_changes() {
	let (mut scope, id) = OperatorFixture::installed().await;
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
async fn disabling_an_approval_still_works_while_compatibility_is_disabled() {
	let (mut scope, id) = OperatorFixture::installed().await;
	scope.state.enabled = false;
	let result = activate(&mut scope, &id, &activation_input(false))
		.await
		.unwrap();
	assert_eq!(result.activation_revision, 1);
	assert!(!scope.inner.calls.iter().any(|c| c == "compatibility"));
}
#[rstest]
#[tokio::test]
async fn denied_catalog_write_cannot_advance_the_active_pointer_or_emit_an_event() {
	let (mut scope, id) = OperatorFixture::installed().await;
	scope.inner.denied = Some("set_approval".into());
	let result = activate(&mut scope, &id, &activation_input(true)).await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(scope.inner.installations[&id].activation_revision, 0);
	assert_eq!(scope.events.len(), 0);
}
#[rstest]
#[tokio::test]
async fn adoption_freezes_the_root_overlay_and_retries_without_another_revision() {
	// Arrange
	let mut scope = OperatorFixture::new();
	let mut overlay = scope.inner.entries["source"].clone();
	overlay.config = json!({"instructions":"Effective legacy bytes"});
	scope.overlays.insert("source".into(), overlay.clone());
	scope.config.insert(
		"source".into(),
		json!({"instructions":"Effective legacy bytes"}),
	);
	// Act
	let first = adopt(&mut scope, &validation(), &adoption(), "node")
		.await
		.unwrap();
	let second = adopt(&mut scope, &validation(), &adoption(), "node")
		.await
		.unwrap();
	// Assert: legacy bytes remain intact, while the new immutable revision stays pending.
	assert_eq!(first.id, second.id);
	assert_eq!(second.latest_revision, 1);
	assert_eq!(second.active_revision, None);
	assert_eq!(scope.inner.allocation, 1);
	assert_eq!(
		scope.inner.revisions[&format!("{}:1", first.id)]
			.entry
			.config,
		overlay.config
	);
	assert_eq!(
		scope.inner.entries["source"].config,
		json!({"instructions":"Frozen instructions"})
	);
	assert_eq!(scope.events.len(), 1);
	assert_eq!(scope.events[0].1["actor"], "operator-adoption");
}
#[rstest]
#[tokio::test]
async fn changed_adoption_input_conflicts_before_loading_the_new_source() {
	let mut scope = OperatorFixture::new();
	adopt(&mut scope, &validation(), &adoption(), "node")
		.await
		.unwrap();
	let mut request = adoption();
	request.source.id = "unavailable".into();
	let result = adopt(&mut scope, &validation(), &request, "node").await;
	assert!(matches!(result, Err(Error::Conflict(_))));
	assert_eq!(scope.inner.allocation, 1);
}
#[rstest]
#[case::changed_raw(true)]
#[case::changed_digest(false)]
#[tokio::test]
async fn altered_legacy_package_record_is_rejected_before_staging(#[case] changed_raw: bool) {
	let mut scope = OperatorFixture::new();
	if changed_raw {
		scope.inner.entries.get_mut("source").unwrap().config = json!({"instructions":"Tampered"});
	} else {
		scope.records.get_mut("source").unwrap().1 = "sha256:tampered".into();
	}
	let result = adopt(&mut scope, &validation(), &adoption(), "node").await;
	assert!(matches!(result, Err(Error::Conflict(_))));
	assert_eq!(scope.inner.installations.len(), 0);
	assert_eq!(scope.inner.allocation, 0);
	assert_eq!(scope.events.len(), 0);
}
#[rstest]
#[tokio::test]
async fn imported_provenance_cannot_be_erased_through_legacy_adoption() {
	let mut scope = OperatorFixture::new();
	scope.inner.provenance.insert(
		"source".into(),
		BTreeSet::from([ConsentEdge {
			source: "ancestor".into(),
			redistributor: "owner".into(),
			grant: SourceGrant::Consent,
		}]),
	);
	let result = adopt(&mut scope, &validation(), &adoption(), "node").await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(scope.inner.allocation, 0);
	assert!(!scope.inner.calls.iter().any(|c| c == "insert_version"));
}
#[rstest]
#[tokio::test]
async fn transitive_legacy_overlay_is_rejected_instead_of_freezing_different_executable_content() {
	let mut scope = OperatorFixture::new();
	let mut root = entry("source");
	root.kind = "tool".into();
	root.config = json!({"registry_node":"aidash://node","provider":"integration.agent@1","operation":"invoke","default_alias":"delegate","tier":"integration","transport":{"transport":"agent","node_id":"node","agent":{"id":"dependency","version":"1.0.0"}}});
	scope.inner.entries.insert("source".into(), root);
	let dependency = entry("dependency");
	scope
		.inner
		.entries
		.insert("dependency".into(), dependency.clone());
	scope.overlays.insert(
		"dependency".into(),
		Entry {
			binding_normalization: None,
			config: json!({"instructions":"Different"}),
			..dependency
		},
	);
	scope.package_record("source");
	let result = adopt(&mut scope, &validation(), &adoption(), "node").await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(scope.inner.allocation, 0);
}
#[rstest]
#[tokio::test]
async fn administration_preserves_candidate_progress_and_has_no_subject_actions() {
	let (mut scope, id) = OperatorFixture::installed().await;
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
