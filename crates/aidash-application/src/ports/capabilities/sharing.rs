//! Local delivery adds destination locks and temporary recipient subjects to the file transaction.
use super::files::FileScopePort;
use crate::Result;
use aidash_domain::{
	RunMetadata,
	capabilities::{
		sessions::Area,
		sharing::{Recipient, Share},
	},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
pub struct Limits {
	pub files: usize,
	pub file_bytes: u64,
	pub bytes: u64,
	pub admission: bool,
}
#[async_trait]
pub trait SharingScope: FileScopePort {
	fn node_id(&self) -> &str;
	fn sharing_limits(&self) -> Result<Limits>;
	fn subjects(&self) -> &[String];
	fn set_subjects(&mut self, subjects: Vec<String>);
	async fn recipient(&mut self, source: &Area, recipient: &Recipient) -> Result<Option<Area>>;
	async fn admitted(&mut self, area: &Area, version: &str) -> Result<Option<Uuid>>;
	async fn workspace(&mut self, id: Uuid) -> Result<aidash_domain::policy::Resource>;
	async fn source_authority(&mut self, workspace: Uuid, constraints: &Value) -> Result<()>;
	async fn begin_received(&mut self, area: Uuid, size: u64) -> Result<()>;
	async fn collaboration(&mut self, id: Uuid, area: Uuid, result: Value) -> Result<()>;
	async fn remote_view(&mut self, id: Uuid) -> Result<Value>;
	async fn remote_prepare(
		&mut self,
		run: &RunMetadata,
		area: &Area,
		input: Share,
		digest: &str,
	) -> Result<Value>;
}
