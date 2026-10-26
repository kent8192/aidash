//! Tool effects retain the execution identity and the original idempotency keys.
use crate::Result;
use crate::tenant_secrets::{CallSecret, SecretCall};
use aidash_domain::{
	Artifact, ArtifactInput, HumanRequest, NewTask, Task,
	agent_plugin::{ListedTool, PluginName, RemoteServer, ServerName},
	exposure::DirectSkill,
	provider::ToolSpec,
	registry::{EntityRef, Search, SkillFile},
	tool::ToolConfig,
};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

#[async_trait]
pub trait ToolTransport: Send + Sync {
	/// `secret` is the Tenant Secret the configuration names, already checked
	/// against the endpoint's host. A self-hosted transport sends the value as
	/// the bearer credential; a brokered one sends the call to the broker and
	/// never reads a value.
	async fn invoke(
		&self,
		configuration: &ToolConfig,
		input: Value,
		key: &str,
		secret: Option<&CallSecret>,
	) -> Result<Value>;
}

/// Names one remote server of one Agent Plugin copy a Run takes in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginServer {
	pub plugin: PluginName,
	/// The [`aidash_domain::agent_plugin::AgentPluginRef::digest`] of the copy.
	pub digest: String,
	pub server: ServerName,
}

/// Why a server could not be listed, beyond a refused destination
/// ([`Error::Forbidden`](crate::Error::Forbidden)) and the deadline the caller enforces.
/// [`Display`](std::fmt::Display) is the cause code a Run's findings carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ListFailure {
	#[error("unreachable")]
	Unreachable,
	#[error("initialize_failed")]
	InitializeFailed,
	#[error("invalid_response")]
	InvalidResponse,
}

impl From<ListFailure> for crate::Error {
	fn from(failure: ListFailure) -> Self {
		Self::Port(Box::new(failure))
	}
}

/// A Run's MCP sessions with the remote servers of its Agent Plugins. Implementations keep one
/// session per Run and server, reopen it when it is missing, and never fail a Run for its absence.
///
/// A refused destination is [`Error::Forbidden`](crate::Error::Forbidden). `list` reports every
/// other failure as a [`ListFailure`] (through `From`) so the Run's findings can name its cause.
/// Only [`Error::Invalid`](crate::Error::Invalid) becomes a recoverable tool result in the harness.
#[async_trait]
pub trait PluginSessions: Send + Sync {
	/// `tools/list` over the Run's session, opening it when needed.
	async fn list(
		&self,
		run: Uuid,
		server: &PluginServer,
		config: &RemoteServer,
	) -> Result<Vec<ListedTool>>;
	/// `tools/call` over the Run's session. When the session had to be (re)opened, the
	/// implementation lists tools first and refuses with `Error::Invalid("CAPABILITY_CHANGED: …")`
	/// if the tool's schema digest differs from `schema_digest`. The result mirrors
	/// [`ToolTransport`]: `{"is_error":true,"content":…}` for an MCP error result, otherwise the
	/// serialized result.
	#[allow(clippy::too_many_arguments)]
	async fn call(
		&self,
		run: Uuid,
		server: &PluginServer,
		config: &RemoteServer,
		tool: &str,
		schema_digest: &str,
		arguments: Value,
		key: &str,
	) -> Result<Value>;
	/// Ends every session of the Run.
	async fn close(&self, run: Uuid);
}

/// Implementations bind one execution identity before any read or mutation.
/// Memory and task operations must retain their policy locks and durable writes.
#[async_trait]
pub trait ToolOperations: Send + Sync {
	async fn registered_skills(&self) -> Result<Vec<EntityRef>>;
	async fn skill_files(&self, reference: &EntityRef) -> Result<Vec<SkillFile>>;
	async fn discover(&self, search: &Search) -> Result<Value>;
	async fn create_task(&self, key: &str, input: &NewTask) -> Result<Task>;
	async fn assign(&self, id: Uuid, policy: &str, reason: &str) -> Result<Value>;
	async fn delegate_with_key(
		&self,
		key: &str,
		id: Uuid,
		node: &str,
		agent: &EntityRef,
	) -> Result<Value>;
	async fn artifact(&self, key: &str, input: &ArtifactInput) -> Result<Artifact>;
	async fn message(&self, key: &str, content: &str) -> Result<()>;
	async fn observation(&self, offset: usize, limit: usize) -> Result<Value>;
	async fn read_record_chunk(
		&self,
		kind: &str,
		id: &str,
		offset: usize,
		maximum: usize,
	) -> Result<Value>;
	async fn memory_mutate(
		&self,
		key: &str,
		changes: &[aidash_domain::memory::Change],
	) -> Result<Vec<aidash_domain::memory::Unit>>;
	async fn memory_recall(
		&self,
		key: &str,
		query: &aidash_domain::memory::RecallQuery,
		reflect: bool,
	) -> Result<Value>;
	async fn human_request(&self, kind: &str, prompt: &str, key: &str) -> Result<HumanRequest>;
	/// Model-visible specifications of the Run's aliased tool Bindings, by alias,
	/// exactly as dispatch advertises them (`bound_specification`).
	async fn binding_specifications(&self) -> Result<BTreeMap<String, ToolSpec>>;
	/// Direct Skills pinned for the Run. Empty when Skills are off, nothing is
	/// pinned, or this placement has no local authority.
	async fn direct_skills(&self) -> Result<Vec<DirectSkill>>;
	/// Bytes of one file of a pinned direct Skill; `None` for a file declared
	/// binary at pinning. A Skill digest other than the pinned one is
	/// `Conflict("CAPABILITY_CHANGED")`.
	async fn direct_skill_file(
		&self,
		skill_id: Uuid,
		digest: &str,
		path: &str,
	) -> Result<Option<Vec<u8>>>;
	/// Encoded-byte cap of one Skill asset chunk: the resource profile's `read_bytes`.
	fn skill_read_bytes(&self) -> Result<usize>;
	/// The Run's Tenant's Tenant Secret `name` for one call to `endpoint`.
	/// `call` names the destination, methods and subject the broker must
	/// authorize. A missing or deleted Tenant Secret, or one that does not
	/// record the endpoint's host, is `secret_unavailable`. A placement with no
	/// Secret Store of its own holds none. `key` is the invocation the call
	/// records its Tenant Secret id, version and host on.
	async fn tenant_secret(
		&self,
		_name: &str,
		_endpoint: &str,
		_call: SecretCall,
		_key: &str,
	) -> Result<CallSecret> {
		Err(crate::Error::Invalid(
			aidash_domain::tenant_secrets::UNAVAILABLE.into(),
		))
	}
}
