//! Local delivery adds destination locks and temporary recipient subjects to the file transaction.
use super::files::FileScopePort;
use crate::Result;
use aidash_domain::{
	RunMetadata,
	capabilities::{
		operations::MountedFile,
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
	async fn require_bound_operation(&mut self, run: Uuid, operation: &str) -> Result<()>;

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

/// What a Run executing for another Home needs of its receiving Node to send files back to that
/// Home (ADR 0139, decision 20): the Area it reads, nothing of a recipient. Any `SharingScope`
/// provides it, so the Node's own scope serves both and a test needs only these methods.
#[async_trait]
pub trait HomeShareScope: Send {
	fn share_limits(&self) -> Result<Limits>;
	/// The result of the call with this key, when the same input already ran.
	async fn replay(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>>;
	async fn active_run(&mut self, area: &Area) -> Result<Option<Uuid>>;
	async fn permit(&mut self, area: &Area, action: &str) -> Result<()>;
	async fn read_range(&mut self, file: &MountedFile, offset: u64) -> Result<Vec<u8>>;
}
#[async_trait]
impl<T: SharingScope + ?Sized> HomeShareScope for T {
	fn share_limits(&self) -> Result<Limits> {
		SharingScope::sharing_limits(self)
	}
	async fn replay(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>> {
		FileScopePort::cached(self, key, digest).await
	}
	async fn active_run(&mut self, area: &Area) -> Result<Option<Uuid>> {
		FileScopePort::current_run(self, area).await
	}
	async fn permit(&mut self, area: &Area, action: &str) -> Result<()> {
		FileScopePort::authorize(self, area, action).await
	}
	async fn read_range(&mut self, file: &MountedFile, offset: u64) -> Result<Vec<u8>> {
		FileScopePort::read_chunk(self, file, offset).await
	}
}
