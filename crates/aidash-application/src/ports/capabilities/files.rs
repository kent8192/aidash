//! Verified readers and publication guards keep concrete files and transactions outside use cases.
use crate::Result;
use aidash_domain::{
	RunMetadata,
	capabilities::{
		operations::{FileScope, MountedFile},
		sessions::Area,
	},
	policy::Resource,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
pub struct Limits {
	pub working_bytes: u64,
	pub read_bytes: usize,
	pub search_matches: usize,
	pub search_bytes: usize,
	pub search_seconds: u64,
}
pub enum Action<'a> {
	Package,
	Python,
	Outbound,
	Share,
	Skill(&'a str),
	Patch,
	Shell,
	Control { kind: &'static str, cancel: bool },
}
#[async_trait]
pub trait VerifiedFile: Send {
	async fn read_range(&mut self, offset: u64, limit: u64) -> Result<Vec<u8>>;
	async fn fill_buf(&mut self) -> Result<&[u8]>;
	fn consume(&mut self, count: usize);
}
#[async_trait]
pub trait FileScopePort: Send {
	fn local_node(&self) -> &str;
	fn binding_snapshot(&self) -> Result<&aidash_domain::registry::bindings::BindingSnapshot> {
		Err(crate::Error::Invalid(
			"Run has no admitted Binding snapshot".into(),
		))
	}
	fn limits(&self) -> Result<Limits>;
	fn policy_revision(&self) -> i64;
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	async fn entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry>;
	async fn check_pinned(&mut self, entry: &Entry) -> Result<()>;
	async fn effective(&mut self, reference: &EntityRef) -> Result<Entry>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn serialize_sharing(&mut self) -> Result<()>;
	async fn for_run(&mut self, run: &RunMetadata) -> Result<Area>;
	async fn authorize(&mut self, area: &Area, action: &str) -> Result<()>;
	async fn current_run(&mut self, area: &Area) -> Result<Option<Uuid>>;
	async fn require_current(&mut self, area: &Area, run: &RunMetadata) -> Result<()>;
	async fn execute(
		&mut self,
		action: Action<'_>,
		area: &mut Area,
		input: Value,
		key: &str,
	) -> Result<Value>;
	async fn output(&mut self, area: &Area, id: Uuid) -> Result<Option<(String, i64, String)>>;
	async fn open(&mut self, file: &MountedFile) -> Result<Box<dyn VerifiedFile>>;
	async fn cached(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>>;
	async fn cache(&mut self, key: Uuid, digest: &str, result: &Value) -> Result<()>;
	async fn begin_pending(&mut self, area: Uuid, size: u64) -> Result<()>;
	async fn read_chunk(&mut self, file: &MountedFile, offset: u64) -> Result<Vec<u8>>;
	async fn write_pending(&mut self, bytes: &[u8]) -> Result<()>;
	async fn finish_pending(&mut self, expected: &str) -> Result<(Uuid, String)>;
	async fn message(&mut self, workspace: Uuid, id: Uuid) -> Result<Value>;
	async fn documents(&mut self, entry: &Entry) -> Result<Value>;
	async fn text_file(
		&mut self,
		area: Uuid,
		path: String,
		text: &str,
		scope: FileScope,
		provenance: Value,
	) -> Result<MountedFile>;
	async fn previous_manifest(&mut self, area: &Area) -> Result<Value>;
	async fn supersede(&mut self, area: &Area, file: Uuid) -> Result<()>;
	async fn persist_manifest(&mut self, area: &Area) -> Result<()>;
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()>;
}
