//! Definition admission borrows the current Marketplace authority transaction.
use crate::Result;
use aidash_domain::{
	marketplace::{Installation, Revision, Version},
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
pub trait DefinitionScope: Send {
	fn tenant(&self) -> &str;
	async fn raw(&mut self, reference: &EntityRef) -> Result<Entry>;
	async fn installation(&mut self, id: &str) -> Result<Option<Installation>>;
	async fn revision(&mut self, id: &str, revision: i64) -> Result<Revision>;
	async fn require_installation_read(
		&mut self,
		installation: &Installation,
		revision: i64,
	) -> Result<()>;
	async fn catalog(&mut self, reference: &EntityRef, action: &str) -> Result<Entry>;
	async fn require_export(&mut self, entry: &Entry) -> Result<()>;
	async fn matching_publication(
		&mut self,
		reference: &EntityRef,
		content: &str,
	) -> Result<Option<Version>>;
	async fn require_reference_read(&mut self, id: Uuid) -> Result<()>;
}

/// Read and distribution decisions use the same scope as dependency admission.
#[async_trait]
pub trait DistributionScope: DefinitionScope {
	async fn version(&mut self, key: &str) -> Result<Option<Version>>;
	async fn audience(&mut self, key: &str)
	-> Result<Option<aidash_domain::marketplace::Audience>>;
	async fn consent(&mut self, key: &str) -> Result<Option<aidash_domain::marketplace::Audience>>;
	fn authority(&mut self, name: String, revision: i64);
	fn operation(&mut self, name: &str, resource: serde_json::Value, request: Option<Uuid>);
	/// Snapshot the current context once, before nested dependency reads change it.
	fn package_resource(&self, version: &Version) -> aidash_domain::policy::Resource;
	fn installation_resource(
		&self,
		installation: &Installation,
		revision: Option<i64>,
	) -> aidash_domain::policy::Resource;
	async fn require(
		&mut self,
		resource: &aidash_domain::policy::Resource,
		action: &str,
	) -> Result<()>;
	async fn decide(
		&mut self,
		resource: &aidash_domain::policy::Resource,
		action: &str,
	) -> Result<bool>;
}

/// Writes, authority evidence and outbox effects use the existing distribution lease.
#[async_trait]
pub trait DistributionWriter: DistributionScope {
	async fn distribution(
		&mut self,
		package: &str,
		redistributor: Option<&str>,
	) -> Result<Option<aidash_domain::marketplace::Audience>>;
	async fn save_distribution(
		&mut self,
		package: &str,
		redistributor: Option<&str>,
		audience: &aidash_domain::marketplace::Audience,
	) -> Result<()>;
	async fn event(&mut self, kind: &str, payload: serde_json::Value) -> Result<()>;
}

#[async_trait]
pub trait PublicationScope: DistributionWriter {
	fn actor(&self) -> &str;
	fn audit_resource(&mut self, resource: serde_json::Value);
	async fn provenance(
		&mut self,
		entry: &Entry,
	) -> Result<std::collections::BTreeSet<aidash_domain::marketplace::ConsentEdge>>;
	async fn publication_replay(
		&mut self,
		request: Uuid,
	) -> Result<Option<aidash_domain::marketplace::Replay>>;
	async fn remember_publication(
		&mut self,
		request: Uuid,
		fingerprint: String,
		result: serde_json::Value,
	) -> Result<()>;
	async fn package_kind(&mut self, repository: &str, package: &str) -> Result<Option<String>>;
	async fn insert_version(
		&mut self,
		version: &Version,
		audience: &aidash_domain::marketplace::Audience,
	) -> Result<()>;
}

#[async_trait]
pub trait InstallationRead: DistributionScope {
	/// Acquire the compatibility lease before selected-installation row locks.
	async fn installation_gate(&mut self) -> Result<()>;
	async fn selected_installation(&mut self, id: &str) -> Result<Option<Installation>>;
	fn audit_installation(&mut self, installation: serde_json::Value);
	async fn approved(
		&mut self,
		tenant: &str,
		reference: &aidash_domain::registry::EntityRef,
	) -> Result<bool>;
}
/// Immutable staged records and all side effects borrow one protected transaction.
#[async_trait]
pub trait StagingScope: crate::ports::registry::PrivateKnowledgeRead {
	fn actor(&self) -> &str;
	fn allocate_entry_id(&mut self) -> Uuid;
	async fn persist_revision(
		&mut self,
		installation: &aidash_domain::marketplace::Installation,
		revision: &aidash_domain::marketplace::Revision,
	) -> Result<()>;
	async fn insert_documents(&mut self, entry: &Entry, documents: serde_json::Value)
	-> Result<()>;
	async fn save_provenance(
		&mut self,
		entry: &Entry,
		provenance: &std::collections::BTreeSet<aidash_domain::marketplace::ConsentEdge>,
	) -> Result<()>;
	async fn copy_provenance(
		&mut self,
		entry: &Entry,
		tenant: &str,
	) -> Result<std::collections::BTreeSet<aidash_domain::marketplace::ConsentEdge>>;
	async fn save_copy_provenance(
		&mut self,
		entry: &Entry,
		tenant: &str,
		provenance: &std::collections::BTreeSet<aidash_domain::marketplace::ConsentEdge>,
	) -> Result<()>;
	async fn installed_event(&mut self, payload: serde_json::Value) -> Result<()>;
}
#[async_trait]
pub trait InstallationScope: InstallationRead + StagingScope {
	async fn installation_replay(
		&mut self,
		operation: &str,
		request: Uuid,
	) -> Result<Option<aidash_domain::marketplace::Replay>>;
	async fn remember_installation(
		&mut self,
		operation: &str,
		request: Uuid,
		fingerprint: String,
		result: serde_json::Value,
	) -> Result<()>;
}

/// Candidate reads preserve database tenant filtering and bounded traversal.
#[async_trait]
pub trait MarketplaceRead: InstallationRead {
	async fn version_page(&mut self, after: &str, limit: u64) -> Result<Vec<(String, Version)>>;
	async fn source_references(&mut self, offset: usize, limit: u64) -> Result<Vec<EntityRef>>;
	/// Filter by the current tenant before decoding any persisted document.
	async fn installation_documents(&mut self) -> Result<Vec<serde_json::Value>>;
}

/// Operator maintenance and adoption preserve one browser/tenant/distribution transaction.
#[async_trait]
pub trait OperatorScope: StagingScope {
	fn principal(&self) -> &aidash_domain::identity::Principal;
	async fn load_tenant(&mut self, tenant: &str) -> Result<()>;
	async fn distribution_lock(&mut self, exclusive: bool) -> Result<()>;
	async fn compatibility(&mut self) -> Result<Option<aidash_domain::marketplace::Compatibility>>;
	async fn save_compatibility(
		&mut self,
		state: &aidash_domain::marketplace::Compatibility,
	) -> Result<()>;
	async fn enable_writer(&mut self) -> Result<()>;
	async fn installation(&mut self, id: &str) -> Result<Option<Installation>>;
	async fn revision(&mut self, id: &str, revision: i64) -> Result<Revision>;
	async fn approved(&mut self, tenant: &str, reference: &EntityRef) -> Result<bool>;
	async fn set_approval(
		&mut self,
		tenant: &str,
		reference: &EntityRef,
		expected_revision: i64,
		enabled: bool,
	) -> Result<()>;
	async fn save_installation(&mut self, installation: &Installation) -> Result<()>;
	async fn event(&mut self, kind: &str, payload: serde_json::Value) -> Result<()>;
	async fn adoption_replay(&mut self, key: &str) -> Result<Option<serde_json::Value>>;
	async fn remember_adoption(&mut self, key: &str, record: serde_json::Value) -> Result<()>;
	async fn raw_definition(&mut self, reference: &EntityRef) -> Result<Entry>;
	async fn effective_legacy(&mut self, reference: &EntityRef) -> Result<Entry>;
	async fn package_record(&mut self, reference: &EntityRef) -> Result<Option<(String, String)>>;
	async fn legacy_config(&mut self, reference: &EntityRef) -> Result<Option<serde_json::Value>>;
	async fn provenance(
		&mut self,
		entry: &Entry,
		tenant: &str,
	) -> Result<std::collections::BTreeSet<aidash_domain::marketplace::ConsentEdge>>;
	async fn version(&mut self, key: &str) -> Result<Option<Version>>;
	async fn insert_version(
		&mut self,
		version: &Version,
		audience: &aidash_domain::marketplace::Audience,
	) -> Result<()>;
	async fn revision_documents(
		&mut self,
		tenant: &str,
		offset: usize,
		limit: u64,
	) -> Result<Vec<(serde_json::Value, serde_json::Value)>>;
}

/// Polling and frame handoff share current authority; the handoff caller owns its lease.
#[async_trait]
pub trait EventScope: InstallationRead {
	async fn compatibility_ready(&mut self) -> Result<()>;
	async fn workspace_allowed(&mut self, workspace: Uuid, action: &str) -> Result<bool>;
	async fn workspace_event_visible(&mut self, event: &aidash_domain::Event) -> Result<bool>;
}
/// Provenance propagation borrows the enclosing authorized mutation transaction.
#[async_trait]
pub trait ProvenanceScope: Send {
	async fn raw_definition(&mut self, reference: &EntityRef) -> Result<Entry>;
	async fn provenance(
		&mut self,
		entry: &Entry,
		tenant: &str,
	) -> Result<std::collections::BTreeSet<aidash_domain::marketplace::ConsentEdge>>;
	async fn save_provenance(
		&mut self,
		entry: &Entry,
		edges: &std::collections::BTreeSet<aidash_domain::marketplace::ConsentEdge>,
	) -> Result<()>;
}
