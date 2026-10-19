//! Registry scopes keep reference reads, immutable writes and events atomic.
use crate::Result;
use aidash_domain::{capabilities::CoreCapabilities, provider::ToolSpec, registry::Entry};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

/// Input contracts advertised by the capability runner are shared with prompt budgeting.
pub trait CoreToolCatalog: Send + Sync {
	fn specifications(&self, config: &CoreCapabilities) -> BTreeMap<String, ToolSpec>;
	fn provider_available(
		&self,
		descriptor: &aidash_domain::tool::providers::ToolDescriptor,
	) -> Result<()> {
		if descriptor.tier.needs_approval() {
			return Err(crate::Error::Invalid(
				"PROVIDER_UNAVAILABLE: the Deployment offers no such provider".into(),
			));
		}
		Ok(())
	}
}

/// The caller owns the scope and its authority locks through protected writes.
#[async_trait]
pub trait DefinitionLookup: Send {
	async fn definition(&mut self, id: &str, version: &str) -> Result<Entry>;
	/// Only an explicit Deployment Administrator adapter may import an authenticated public closure.
	async fn foreign_agent(
		&mut self,
		_: &str,
		_: &aidash_domain::registry::bindings::QualifiedRef,
	) -> Result<aidash_domain::registry::bindings::ForeignAgentSnapshot> {
		Err(crate::Error::Forbidden)
	}
	/// The Tenant's stored Agent Plugin copy with this tree digest, as registration and
	/// admission read it. A lookup that never stores plugins refuses.
	async fn agent_plugin(
		&mut self,
		_: &str,
	) -> Result<Option<super::bindings::StoredAgentPlugin>> {
		Err(crate::Error::Invalid("AGENT_PLUGIN_UNAVAILABLE".into()))
	}
	/// Exact installed references must be active and individually approved for
	/// new registration/admission. A default adapter never invents that authority.
	async fn binding_installation(
		&mut self,
		_: &aidash_domain::registry::Projection,
	) -> Result<()> {
		Err(crate::Error::Forbidden)
	}
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

/// Idempotency reservation, the authority check, the immutable insert and its
/// event use this same scope.
#[async_trait]
pub trait RegistrationScope: DefinitionWriter {
	async fn assign_id(&mut self, entry: &mut Entry, key: Option<Uuid>) -> Result<()>;
	/// Whether the requester may create this definition or version in its
	/// Tenant (`registry.create`), decided in the same transaction as the write.
	async fn authorize(&mut self, entry: &Entry) -> Result<()>;
	async fn append_event(&mut self, kind: &str, payload: Value) -> Result<()>;
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

/// Names the Agent Plugin to import from a public GitHub URL.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginImportRequest {
	pub url: String,
	/// A plugin directory from an earlier `Choose` answer, relative to the repository root.
	pub plugin_path: Option<String>,
}

/// One plugin directory at an exact commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedPlugin {
	/// The plugin directory relative to the repository root; empty for the root.
	pub root: String,
	/// `https://github.com/<owner>/<repo>/tree/<commit sha>[/<root>]`.
	pub source: String,
	/// Paths relative to the plugin root, sorted.
	pub files: Vec<aidash_domain::agent_plugin::PluginFile>,
	/// Paths left out because the Agent Plugins specification forbids access to them, sorted.
	pub denied: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginFetch {
	/// The repository holds several plugins; these are their roots.
	Choose(Vec<String>),
	Fetched(FetchedPlugin),
}

/// Fetch a bounded plugin directory from a permitted public source.
#[async_trait::async_trait]
pub trait AgentPluginSource: Send + Sync {
	async fn fetch(&self, request: PluginImportRequest) -> crate::Result<PluginFetch>;
}

pub mod workbench;
