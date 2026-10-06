//! Tool effects retain the execution identity and the original idempotency keys.
use crate::Result;
use aidash_domain::{
	Artifact, ArtifactInput, HumanRequest, NewTask, Task,
	registry::{EntityRef, Search, SkillFile},
	tool::ToolConfig,
};
use async_trait::async_trait;
use serde_json::Value;
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
	async fn remember(&self, input: &Value) -> Result<()>;
	async fn human_request(&self, kind: &str, prompt: &str, key: &str) -> Result<HumanRequest>;
}
