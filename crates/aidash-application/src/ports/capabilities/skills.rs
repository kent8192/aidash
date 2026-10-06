//! Skill objects, pinned records and current context authority share the caller's transaction.
use super::files::FileScopePort;
use crate::Result;
use aidash_domain::{
	RunMetadata,
	capabilities::{operations::MountedFile, records::Record},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
pub struct Limits {
	pub files: usize,
	pub bytes: usize,
}
#[async_trait]
pub trait SkillScope: FileScopePort {
	fn skill_limits(&self) -> Result<Limits>;
	async fn read_skill_file(&mut self, file: &MountedFile) -> Result<Vec<u8>>;
	async fn put_skill(&mut self, area: Uuid, bytes: &[u8]) -> Result<(Uuid, String)>;
	async fn skill_record(&mut self, run: Uuid) -> Result<Record>;
	async fn insert_skills(&mut self, run: Uuid, area: Uuid, data: Value) -> Result<()>;
	async fn update_skills(&mut self, record: &mut Record) -> Result<()>;
	async fn context_authority(&mut self, run: &RunMetadata) -> Result<()>;
}
#[async_trait]
pub trait SkillHeadroom: Send {
	async fn pinned_data(&mut self, run: Uuid) -> Result<Option<Value>>;
}
