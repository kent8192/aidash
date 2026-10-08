use super::*;
use crate::ports::marketplace::{DefinitionScope, DistributionScope};
use crate::ports::{Credentials, registry::CoreToolCatalog};
use aidash_domain::policy::Resource;
use aidash_domain::registry::Entry;
use async_trait::async_trait;
use rstest::rstest;
use std::{
	collections::{BTreeMap, BTreeSet},
	sync::Arc,
};

struct UnusedCredentials;
impl Credentials for UnusedCredentials {
	fn resolve(&self, _: &str) -> Result<String> {
		panic!("structural staging never resolves a local credential")
	}
}
struct CoreContracts;
impl CoreToolCatalog for CoreContracts {
	fn specifications(
		&self,
		_: &aidash_domain::capabilities::CoreCapabilities,
	) -> BTreeMap<String, aidash_domain::provider::ToolSpec> {
		BTreeMap::new()
	}
}
pub(crate) fn validation() -> DefinitionValidation {
	DefinitionValidation::new(Arc::new(UnusedCredentials), Arc::new(CoreContracts))
}
pub(crate) fn entry(id: &str) -> Entry {
	serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":"skill","name":{"en":id},"description":{"en":"Fixture"},"config":{"instructions":"Frozen instructions"}})).unwrap()
}

#[derive(Default)]
pub(crate) struct Fixture {
	pub(crate) entries: BTreeMap<String, Entry>,
	pub(crate) versions: BTreeMap<String, Version>,
	pub(crate) audiences: BTreeMap<String, Audience>,
	pub(crate) grants: BTreeMap<String, Audience>,
	pub(crate) revisions: BTreeMap<String, Revision>,
	pub(crate) installations: BTreeMap<String, Installation>,
	pub(crate) documents: BTreeMap<String, Value>,
	pub(crate) requests: BTreeMap<(String, Uuid), Replay>,
	pub(crate) copies: BTreeSet<ConsentEdge>,
	pub(crate) provenance: BTreeMap<String, BTreeSet<ConsentEdge>>,
	pub(crate) allocation: u128,
	pub(crate) revision_only: bool,
	pub(crate) calls: Vec<String>,
	pub(crate) authority: Vec<(String, i64)>,
	pub(crate) denied: Option<String>,
	pub(crate) hidden: BTreeSet<String>,
	pub(crate) source_refs: Vec<EntityRef>,
	pub(crate) source_pages: Vec<(usize, u64)>,
}
impl Fixture {
	fn new() -> Self {
		Self {
			entries: BTreeMap::from([("source".into(), entry("source"))]),
			..Default::default()
		}
	}
	pub(crate) fn record(&mut self, action: &str) -> Result<()> {
		self.calls.push(action.into());
		if self.denied.as_deref() == Some(action) {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	fn resource(&self, kind: &str, id: &str) -> Resource {
		Resource {
			tenant: "owner".into(),
			kind: kind.into(),
			id: id.into(),
			attributes: json!({}),
		}
	}
}
#[async_trait]
impl DefinitionScope for Fixture {
	fn tenant(&self) -> &str {
		"owner"
	}
	async fn raw(&mut self, reference: &EntityRef) -> Result<Entry> {
		self.record("raw")?;
		self.entries
			.get(&reference.id)
			.cloned()
			.ok_or(Error::Forbidden)
	}
	async fn installation(&mut self, id: &str) -> Result<Option<Installation>> {
		Ok(self.installations.get(id).cloned())
	}
	async fn revision(&mut self, id: &str, revision: i64) -> Result<Revision> {
		self.revisions
			.get(&format!("{id}:{revision}"))
			.cloned()
			.ok_or(Error::Forbidden)
	}
	async fn require_installation_read(&mut self, _: &Installation, _: i64) -> Result<()> {
		self.record("installation.read")
	}
	async fn catalog(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		self.record(action)?;
		if self.hidden.contains(&reference.id) {
			return Err(Error::NotFound(reference.id.clone()));
		}
		self.entries
			.get(&reference.id)
			.cloned()
			.ok_or(Error::Forbidden)
	}
	async fn require_export(&mut self, _: &Entry) -> Result<()> {
		self.record("registry.export")
	}
	async fn matching_publication(&mut self, _: &EntityRef, _: &str) -> Result<Option<Version>> {
		Ok(None)
	}
	async fn require_reference_read(&mut self, _: Uuid) -> Result<()> {
		self.record("reference.read")
	}
}
#[async_trait]
impl DistributionScope for Fixture {
	async fn version(&mut self, key: &str) -> Result<Option<Version>> {
		self.record("version")?;
		Ok(self.versions.get(key).cloned())
	}
	async fn audience(&mut self, key: &str) -> Result<Option<Audience>> {
		Ok(self.audiences.get(key).cloned())
	}
	async fn consent(&mut self, key: &str) -> Result<Option<Audience>> {
		Ok(self.grants.get(key).cloned())
	}
	fn authority(&mut self, name: String, revision: i64) {
		self.authority.push((name, revision));
	}
	fn operation(&mut self, name: &str, _: Value, _: Option<Uuid>) {
		self.calls.push(format!("operation:{name}"));
	}
	fn package_resource(&self, version: &Version) -> Resource {
		self.resource("package", &version.key)
	}
	fn installation_resource(
		&self,
		installation: &Installation,
		revision: Option<i64>,
	) -> Resource {
		Resource {
			attributes: json!({"revision":revision}),
			..self.resource("installation", &installation.id)
		}
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		if action == "marketplace.browse" && self.hidden.contains(&resource.id) {
			return Err(Error::Forbidden);
		}
		if self.revision_only && resource.attributes["revision"].is_null() {
			self.calls.push(action.into());
			return Ok(());
		}
		self.record(action)
	}
	async fn decide(&mut self, _: &Resource, action: &str) -> Result<bool> {
		self.record(action)?;
		Ok(true)
	}
}

#[async_trait]
impl InstallationRead for Fixture {
	async fn installation_gate(&mut self) -> Result<()> {
		self.record("installation_gate")
	}
	async fn selected_installation(&mut self, id: &str) -> Result<Option<Installation>> {
		self.record("selected_installation")?;
		Ok(self.installations.get(id).cloned())
	}
	fn audit_installation(&mut self, _: Value) {
		self.calls.push("audit_installation".into());
	}
	async fn approved(&mut self, _: &str, _: &EntityRef) -> Result<bool> {
		Ok(false)
	}
}
#[async_trait]
impl crate::ports::registry::PrivateKnowledgeRead for Fixture {
	async fn documents(&mut self, entry: &Entry) -> Result<Option<Value>> {
		self.record("documents")?;
		Ok(self.documents.get(&entry.id).cloned())
	}
}
#[async_trait]
impl StagingScope for Fixture {
	fn actor(&self) -> &str {
		"publisher"
	}
	fn allocate_entry_id(&mut self) -> Uuid {
		let id = Uuid::from_u128(self.allocation);
		self.allocation += 1;
		id
	}
	async fn persist_revision(
		&mut self,
		install: &Installation,
		revision: &Revision,
	) -> Result<()> {
		self.record("persist_revision")?;
		self.installations
			.insert(install.id.clone(), install.clone());
		self.entries
			.insert(revision.entry.id.clone(), revision.entry.clone());
		self.revisions.insert(
			format!("{}:{}", install.id, revision.revision),
			revision.clone(),
		);
		Ok(())
	}
	async fn insert_documents(&mut self, entry: &Entry, documents: Value) -> Result<()> {
		self.record("insert_documents")?;
		self.documents.insert(entry.id.clone(), documents);
		Ok(())
	}
	async fn save_provenance(
		&mut self,
		entry: &Entry,
		provenance: &BTreeSet<ConsentEdge>,
	) -> Result<()> {
		self.record("save_provenance")?;
		self.provenance.insert(entry.id.clone(), provenance.clone());
		Ok(())
	}
	async fn copy_provenance(&mut self, _: &Entry, _: &str) -> Result<BTreeSet<ConsentEdge>> {
		self.record("copy_provenance")?;
		Ok(self.copies.clone())
	}
	async fn save_copy_provenance(
		&mut self,
		_: &Entry,
		_: &str,
		provenance: &BTreeSet<ConsentEdge>,
	) -> Result<()> {
		self.record("save_copy_provenance")?;
		self.copies = provenance.clone();
		Ok(())
	}
	async fn installed_event(&mut self, _: Value) -> Result<()> {
		self.record("event:marketplace.installed")
	}
}
#[async_trait]
impl InstallationScope for Fixture {
	async fn installation_replay(
		&mut self,
		operation: &str,
		request: Uuid,
	) -> Result<Option<Replay>> {
		Ok(self.requests.get(&(operation.into(), request)).cloned())
	}
	async fn remember_installation(
		&mut self,
		operation: &str,
		request: Uuid,
		fingerprint: String,
		result: Value,
	) -> Result<()> {
		self.record("remember_installation")?;
		self.requests.insert(
			(operation.into(), request),
			Replay {
				fingerprint,
				result,
			},
		);
		Ok(())
	}
}
pub(crate) fn published() -> Fixture {
	let mut scope = Fixture::new();
	let entity = scope.entries["source"].clone();
	let package = aidash_domain::registry::Package {
		entity: entity.clone(),
		author: "Fixture".into(),
		permissions: vec![],
		dependencies: vec![],
	};
	let manifest_source = serde_json::to_string(&package).unwrap();
	use sha2::{Digest, Sha256};
	let version = Version {
		key: "package".into(),
		repository: "node".into(),
		owner_tenant: "owner".into(),
		package_id: "package".into(),
		version: entity.version.clone(),
		kind: entity.kind.clone(),
		publisher: "publisher".into(),
		source: reference(&entity),
		digest: format!("sha256:{:x}", Sha256::digest(manifest_source.as_bytes())),
		manifest_source,
		dependencies: vec![],
		lineage: BTreeSet::new(),
	};
	scope.audiences.insert(
		version.key.clone(),
		aidash_domain::marketplace::publication::initial_audience(&version),
	);
	scope.versions.insert(version.key.clone(), version);
	scope
}
pub(crate) fn input(scope: &Fixture) -> InstallCommand {
	InstallCommand {
		digest: scope.versions["package"].digest.clone(),
		config: json!({}),
		bindings: vec![],
		idempotency_key: Uuid::nil(),
	}
}

#[rstest]
#[tokio::test]
async fn installation_stays_pending_and_a_retry_reuses_the_same_revision() {
	// Arrange
	let mut scope = published();
	let input = input(&scope);
	// Act
	let installed = install(&mut scope, &validation(), "package", &input, "node")
		.await
		.unwrap();
	scope.calls.clear();
	let retry = install(&mut scope, &validation(), "package", &input, "node")
		.await
		.unwrap();
	// Assert
	assert_eq!(installed.entry, retry.entry);
	assert_eq!(installed.digest, retry.digest);
	assert_eq!(retry.installation.latest_revision, 1);
	assert_eq!(retry.installation.active_revision, None);
	assert!(!retry.approved);
	assert_eq!(scope.allocation, 1);
	assert!(
		!scope
			.calls
			.iter()
			.any(|c| c == "persist_revision" || c == "event:marketplace.installed")
	);
	assert!(scope.calls.iter().any(|c| c == "marketplace.install"));
	assert!(scope.calls.iter().any(|c| c == "installation.read"));
}
#[rstest]
#[tokio::test]
async fn unchanged_configuration_with_an_older_revision_does_not_create_new_pending_work() {
	// Arrange
	let mut scope = published();
	let initial_input = input(&scope);
	let installed = install(&mut scope, &validation(), "package", &initial_input, "node")
		.await
		.unwrap();
	let changed = ConfigureCommand {
		expected_revision: 1,
		config: json!({"instructions":"Changed instructions"}),
		bindings: vec![],
		idempotency_key: Uuid::from_u128(1),
	};
	let second = configure(
		&mut scope,
		&validation(),
		&installed.installation.id,
		&changed,
		"node",
	)
	.await
	.unwrap();
	scope.calls.clear();
	let mut retry = changed;
	retry.idempotency_key = Uuid::from_u128(2);
	// Act
	let unchanged = configure(
		&mut scope,
		&validation(),
		&installed.installation.id,
		&retry,
		"node",
	)
	.await
	.unwrap();
	// Assert
	assert_eq!(second.revision, 2);
	assert_eq!(unchanged.revision, 2);
	assert_eq!(unchanged.entry, second.entry);
	assert_eq!(scope.allocation, 2);
	assert!(
		!scope
			.calls
			.iter()
			.any(|c| c == "persist_revision" || c == "event:marketplace.installed")
	);
	assert_eq!(unchanged.installation.active_revision, None);
}
#[rstest]
#[tokio::test]
async fn revoked_revision_hides_the_diagnostic_for_a_changed_replay_input() {
	// Arrange
	let mut scope = published();
	let mut input = input(&scope);
	install(&mut scope, &validation(), "package", &input, "node")
		.await
		.unwrap();
	input.config = json!({"instructions":"Changed input"});
	scope.denied = Some("installation.read".into());
	scope.revision_only = true;
	scope.calls.clear();
	// Act
	let result = install(&mut scope, &validation(), "package", &input, "node").await;
	// Assert
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(scope.allocation, 1);
	assert!(
		!scope
			.calls
			.iter()
			.any(|c| c == "persist_revision" || c == "event:marketplace.installed")
	);
}
#[rstest]
#[case::matching(true)]
#[case::tampered(false)]
#[tokio::test]
async fn private_document_copy_precedes_provenance_and_requires_the_frozen_digest(
	#[case] matching: bool,
) {
	// Arrange
	let mut scope = published();
	let mut source = scope.versions["package"].clone();
	let documents = json!([{"name":"Private text","media_type":"text/plain","text":"Retained immutable reference"}]);
	scope.documents.insert("source".into(), documents.clone());
	let mut agent = entry("source");
	agent.kind = "source".into();
	agent.config = json!({"schema_version":1,"source":{"adapter":"private_references","digest":aidash_domain::registry::knowledge::digest(&documents)}});
	let mut package = manifest(&source).unwrap();
	package.entity = agent.clone();
	source.kind = "source".into();
	source.manifest_source = serde_json::to_string(&package).unwrap();
	use sha2::{Digest, Sha256};
	source.digest = format!(
		"sha256:{:x}",
		Sha256::digest(source.manifest_source.as_bytes())
	);
	if !matching {
		agent.config["source"]["digest"] = json!("0".repeat(64));
	}
	let installation = prospective("owner", "package");
	// Act
	let result = stage(
		&mut scope,
		&validation(),
		installation,
		&source,
		json!({}),
		(agent, vec![], vec![]),
	)
	.await;
	// Assert
	if matching {
		assert_eq!(result.unwrap(), 1);
		assert_eq!(
			scope.calls,
			vec![
				"persist_revision",
				"documents",
				"insert_documents",
				"save_provenance",
				"copy_provenance",
				"save_copy_provenance",
				"event:marketplace.installed"
			]
		);
		assert_eq!(
			scope.documents["mkt-00000000000000000000000000000000"],
			documents
		);
		assert_eq!(
			scope.provenance["mkt-00000000000000000000000000000000"].len(),
			2
		);
	} else {
		assert!(matches!(result, Err(Error::Forbidden)));
		assert_eq!(scope.calls, vec!["persist_revision", "documents"]);
		assert!(scope.provenance.is_empty());
	}
}
#[rstest]
#[tokio::test]
async fn deterministic_staging_retains_source_version_and_current_active_pointer() {
	// Arrange
	let scope = published();
	let mut installation = prospective("owner", "package");
	installation.latest_revision = 4;
	installation.active_revision = Some(3);
	installation.activation_revision = 9;
	let source = &scope.versions["package"];
	let mut entity = entry("source");
	entity.version = "9.8.7".into();
	// Act
	let staged = stage_revision(
		installation,
		source,
		json!({"local":true}),
		(entity, vec![], vec![]),
		Uuid::nil(),
	);
	// Assert
	assert_eq!(staged.installation.latest_revision, 5);
	assert_eq!(staged.installation.active_revision, Some(3));
	assert_eq!(staged.installation.activation_revision, 9);
	assert_eq!(
		staged.revision.entry.id,
		"mkt-00000000000000000000000000000000"
	);
	assert_eq!(staged.revision.entry.version, "1.0.0");
	assert_eq!(
		staged
			.revision
			.entry
			.installation
			.as_ref()
			.unwrap()
			.revision,
		5
	);
	assert_eq!(staged.revision.source.version, source.version);
	assert_eq!(staged.revision.digest, key(&staged.revision.entry));
	assert_eq!(staged.provenance.len(), 2);
}

#[async_trait]
impl crate::ports::marketplace::MarketplaceRead for Fixture {
	async fn version_page(&mut self, after: &str, limit: u64) -> Result<Vec<(String, Version)>> {
		Ok(self
			.versions
			.iter()
			.filter(|(key, _)| key.as_str() > after)
			.take(limit as usize)
			.map(|(key, version)| (key.clone(), version.clone()))
			.collect())
	}
	async fn source_references(&mut self, offset: usize, limit: u64) -> Result<Vec<EntityRef>> {
		self.source_pages.push((offset, limit));
		Ok(self
			.source_refs
			.iter()
			.skip(offset)
			.take(limit as usize)
			.cloned()
			.collect())
	}
	async fn installation_documents(&mut self) -> Result<Vec<Value>> {
		self.installations
			.values()
			.filter(|i| i.tenant == "owner")
			.map(|i| serde_json::to_value(i).map_err(Into::into))
			.collect()
	}
}

#[rstest]
#[case::wrong_tenant("other", 1, false, false, false)]
#[case::unknown_contract("owner", 2, false, false, false)]
#[case::disabled_gate("owner", 1, true, true, false)]
#[case::missing_selected_row("owner", 1, false, false, false)]
#[case::retained_inactive_revision("owner", 1, true, false, false)]
#[case::selected_active_revision("owner", 1, true, false, true)]
#[tokio::test]
async fn active_admission_rechecks_gate_then_selected_row(
	#[case] tenant: &str,
	#[case] contract: u8,
	#[case] has_row: bool,
	#[case] denied_gate: bool,
	#[case] selected: bool,
) {
	// Arrange
	let mut scope = published();
	let installation = prospective("owner", "package");
	let mut projected = entry("projected");
	projected.installation = Some(aidash_domain::registry::Projection {
		tenant: tenant.into(),
		installation: installation.id.clone(),
		revision: 2,
		contract,
	});
	if has_row {
		scope.installations.insert(
			installation.id.clone(),
			Installation {
				active_revision: selected.then_some(2),
				..installation
			},
		);
	}
	if denied_gate {
		scope.denied = Some("installation_gate".into());
	}
	// Act
	let result = active(&mut scope, &projected).await.unwrap();
	// Assert: invalid projection metadata does not acquire a lease or decode rows.
	assert_eq!(result, selected);
	assert_eq!(
		scope.calls,
		if tenant != "owner" || contract != 1 {
			vec![]
		} else if denied_gate {
			vec!["installation_gate"]
		} else {
			vec!["installation_gate", "selected_installation"]
		}
	);
}
#[rstest]
#[tokio::test]
async fn unprojected_definitions_need_no_marketplace_gate() {
	let mut scope = published();
	scope.denied = Some("installation_gate".into());
	let active = active(&mut scope, &entry("native")).await.unwrap();
	assert!(active);
	assert_eq!(scope.calls, Vec::<String>::new());
}

#[async_trait]
impl crate::ports::marketplace::EventScope for Fixture {
	async fn compatibility_ready(&mut self) -> Result<()> {
		if self.denied.as_deref() == Some("compatibility_external") {
			return Err(Error::External("fixture connection failure".into()));
		}
		self.record("compatibility_ready")
	}
	async fn workspace_allowed(&mut self, _: Uuid, action: &str) -> Result<bool> {
		self.calls.push(action.into());
		Ok(self.denied.as_deref() != Some(action))
	}
	async fn workspace_event_visible(&mut self, _: &aidash_domain::Event) -> Result<bool> {
		self.calls.push("workspace_event_visible".into());
		Ok(self.denied.as_deref() != Some("workspace_event_visible"))
	}
}
#[async_trait]
impl crate::ports::marketplace::ProvenanceScope for Fixture {
	async fn raw_definition(&mut self, reference: &EntityRef) -> Result<Entry> {
		DefinitionScope::raw(self, reference).await
	}
	async fn provenance(&mut self, entry: &Entry, _: &str) -> Result<BTreeSet<ConsentEdge>> {
		let mut edges = self.provenance.get(&entry.id).cloned().unwrap_or_default();
		edges.extend(self.copies.clone());
		Ok(edges)
	}
	async fn save_provenance(
		&mut self,
		entry: &Entry,
		edges: &BTreeSet<ConsentEdge>,
	) -> Result<()> {
		StagingScope::save_provenance(self, entry, edges).await
	}
}
#[rstest]
#[case::known_provenance(true)]
#[case::native_untracked(false)]
#[tokio::test]
async fn provenance_propagation_preserves_ancestor_edges_without_manufacturing_empty_records(
	#[case] tracked: bool,
) {
	let mut scope = published();
	let edge = ConsentEdge {
		source: "ancestor".into(),
		redistributor: "owner".into(),
		grant: SourceGrant::Consent,
	};
	if tracked {
		scope.copies.insert(edge.clone());
	}
	propagate_provenance(
		&mut scope,
		&EntityRef {
			id: "source".into(),
			version: "1.0.0".into(),
		},
		&entry("target"),
		"owner",
	)
	.await
	.unwrap();
	assert_eq!(
		scope.provenance.get("target").cloned(),
		tracked.then(|| BTreeSet::from([edge]))
	);
	assert_eq!(
		scope
			.calls
			.iter()
			.filter(|c| c.as_str() == "save_provenance")
			.count(),
		usize::from(tracked)
	);
}
