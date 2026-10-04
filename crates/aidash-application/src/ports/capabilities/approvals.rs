//! Approvals share the caller's live policy, subject chain, record locks and event transaction.
use crate::Result;
use aidash_domain::{
	RunMetadata,
	capabilities::{operations::MountedFile, records::Record},
	policy::{Evaluation, PolicyBundle, Resource},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;
pub struct Limits {
	pub admission: bool,
	pub origins: Vec<String>,
	pub approval_seconds: u64,
	pub grant_seconds: u64,
}
pub struct Creation {
	pub id: Uuid,
	pub area: Option<Uuid>,
	pub kind: &'static str,
	pub state: &'static str,
	pub data: Value,
	pub expires: Option<DateTime<Utc>>,
}
#[async_trait]
pub trait ApprovalScope: Send {
	fn principal(&self) -> &str;
	fn credential(&self) -> Uuid;
	fn subjects(&self) -> &[String];
	fn replace_subjects(&mut self, subjects: Vec<String>) -> Vec<String>;
	fn policy_revision(&self) -> i64;
	fn bundle(&self) -> &PolicyBundle;
	fn evaluation(&self, subject: &str, target: &Resource, action: &str) -> Evaluation;
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	fn limits(&self) -> Result<Limits>;
	async fn run_context(&mut self, id: Uuid) -> Result<RunMetadata>;
	async fn interaction_run(&mut self, id: Uuid) -> Result<RunMetadata>;
	async fn context_authority(&mut self) -> Result<()>;
	async fn active_run(&mut self) -> Result<Option<Uuid>>;
	async fn require(&mut self, target: &Resource, action: &str) -> Result<()>;
	async fn load(&mut self, id: Uuid, kind: &str) -> Result<Record>;
	async fn grants(&mut self, run: Uuid) -> Result<Vec<Record>>;
	async fn insert(&mut self, creation: Creation) -> Result<Record>;
	async fn transfer_owner(&mut self, id: Uuid, owner: &str) -> Result<()>;
	async fn update(&mut self, record: &mut Record) -> Result<()>;
	async fn cached(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>>;
	async fn cache(&mut self, key: Uuid, digest: &str, value: &Value) -> Result<()>;
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()>;
	async fn read(&mut self, file: &MountedFile) -> Result<Vec<u8>>;
}
