//! Tool effects retain the execution identity and the original idempotency keys.
use crate::Result;
use aidash_domain::{
	Artifact, ArtifactInput, HumanRequest, NewTask, Task,
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
	async fn invoke(&self, configuration: &ToolConfig, input: Value, key: &str) -> Result<Value>;
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
	/// Bytes of one file of a pinned direct Skill. A Skill digest other than the
	/// pinned one is `Conflict("CAPABILITY_CHANGED")`.
	async fn direct_skill_file(&self, skill_id: Uuid, digest: &str, path: &str) -> Result<Vec<u8>>;
	/// Encoded-byte cap of one Skill asset chunk: the resource profile's `read_bytes`.
	fn skill_read_bytes(&self) -> Result<usize>;
}
