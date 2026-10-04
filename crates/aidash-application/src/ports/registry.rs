//! Registry scopes keep reference reads, immutable writes and events atomic.
use crate::Result;
use aidash_domain::{
	capabilities::CoreCapabilities,
	provider::ToolSpec,
	registry::{Entry, PackageRecord},
};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

/// Input contracts advertised by the capability runner are shared with prompt budgeting.
pub trait CoreToolCatalog: Send + Sync {
	fn specifications(&self, config: &CoreCapabilities) -> BTreeMap<String, ToolSpec>;
}

/// The caller owns the scope and its authority locks through protected writes.
#[async_trait]
pub trait DefinitionLookup: Send {
	async fn definition(&mut self, id: &str, version: &str) -> Result<Entry>;
	async fn overrides(&mut self, id: &str, version: &str) -> Result<Option<Value>>;
	async fn executor_kind(&mut self, id: &str, version: &str) -> Result<Option<String>> {
		Ok(Some(self.definition(id, version).await?.kind))
	}
}

#[async_trait]
pub trait RegistryRead: DefinitionLookup {
	async fn definitions(
		&mut self,
		kind: Option<&str>,
		offset: usize,
		limit: Option<usize>,
	) -> Result<Vec<DefinitionDocument>>;
	async fn generated(&mut self, id: &str, version: &str) -> Result<bool>;
}

/// Storage identity is retained before decoding metadata or excluding installations.
pub struct DefinitionDocument {
	pub id: String,
	pub version: String,
	pub metadata: Value,
}

#[async_trait]
pub trait DefinitionWriter: DefinitionLookup {
	async fn insert_definition(&mut self, entry: &Entry) -> Result<bool>;
}

/// Idempotency reservation, immutable insert and its event use this same scope.
#[async_trait]
pub trait RegistrationScope: DefinitionWriter {
	async fn assign_id(&mut self, entry: &mut Entry, key: Option<Uuid>) -> Result<()>;
	async fn append_event(&mut self, kind: &str, payload: Value) -> Result<()>;
}

/// Implementations must retain definition locks and immutable digest comparisons.
#[async_trait]
pub trait PackageScope: DefinitionLookup {
	async fn publish(
		&mut self,
		id: &str,
		version: &str,
		manifest: Value,
		digest: &str,
		source: &str,
	) -> Result<(PackageRecord, bool)>;
	async fn install(&mut self, entry: &Entry, digest: &str, config: Value) -> Result<bool>;
	async fn append_event(&mut self, kind: &str, payload: Value) -> Result<()>;
}

/// Manifest bytes are checked against both the advertised digest and decoded value.
pub struct PackageSnapshot {
	pub manifest: Value,
	pub source: String,
	pub digest: String,
}

/// A definition's private reference text stays separate from discoverable metadata.
#[async_trait]
pub trait PrivateKnowledgeRead: Send {
	async fn documents(&mut self, entry: &Entry) -> Result<Option<Value>>;
}

/// Definition reservation, private contents and event publication commit together.
#[async_trait]
pub trait PrivateKnowledgeScope: RegistrationScope + PrivateKnowledgeRead {
	async fn insert_documents(&mut self, entry: &Entry, documents: Value) -> Result<()>;
}

/// Import a bounded snapshot from a permitted public Skill source.
#[async_trait::async_trait]
pub trait SkillSource: Send + Sync {
	async fn import(
		&self,
		request: aidash_domain::registry::skill_import::ImportRequest,
	) -> crate::Result<aidash_domain::registry::skill_import::ImportResult>;
}
