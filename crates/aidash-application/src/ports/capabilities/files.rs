//! Verified readers and publication guards keep concrete files and transactions outside use cases.
use crate::Result;
use aidash_domain::{
	RunMetadata,
	capabilities::{operations::MountedFile, sessions::Area},
	policy::Resource,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeSet;
use uuid::Uuid;
pub struct Limits {
	pub patch_bytes: usize,
	pub read_bytes: usize,
	pub search_matches: usize,
	pub search_bytes: usize,
	pub search_seconds: u64,
}
pub enum Action<'a> {
	Outbound,
	Skill(&'a str),
	/// A Completion Gate Check: shell admission, no file publication (#112).
	Verify,
	Control {
		kind: &'static str,
		cancel: bool,
	},
	/// Admit a `shell` command or `code_interpreter` cell for the Execution Pod
	/// of the Run's Scratch Space (#745).
	PodCommand {
		kind: &'static str,
		scratch_space_id: String,
	},
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
	async fn for_run(&mut self, run: &RunMetadata) -> Result<Area>;
	/// The Execution Pod of the Run's Scratch Space, when this Deployment runs
	/// Agent tools there (#745). None keeps the tools on the Runner's
	/// per-command path.
	async fn pod(
		&mut self,
		_run: &RunMetadata,
	) -> Result<Option<super::pod_operations::PodContext>> {
		Ok(None)
	}
	/// Recheck Scratch Space access and retain it through detached file settlement.
	async fn recheck_pod(
		&mut self,
		run: &RunMetadata,
	) -> Result<Option<super::pod_operations::PodContext>> {
		self.pod(run).await
	}
	async fn authorize(&mut self, area: &Area, action: &str) -> Result<()>;
	async fn execute(
		&mut self,
		action: Action<'_>,
		area: &mut Area,
		input: Value,
		key: &str,
	) -> Result<Value>;
	async fn output(&mut self, area: &Area, id: Uuid) -> Result<Option<(String, i64, String)>>;
	async fn open(&mut self, file: &MountedFile) -> Result<Box<dyn VerifiedFile>>;
	/// Authorize an immutable object before a detached range transfer.
	async fn open_ranges(&mut self, file: &MountedFile) -> Result<Box<dyn VerifiedFile>> {
		self.open(file).await
	}
	/// Of these Reference records, the ids that can no longer be used because the
	/// file was revoked, expired or removed (ADR 0114). One query however many ids.
	async fn bound_references(
		&mut self,
		_config: &aidash_domain::registry::AgentConfig,
	) -> Result<Vec<MountedFile>> {
		Ok(Vec::new())
	}
	async fn unusable_references(&mut self, ids: &[Uuid]) -> Result<BTreeSet<Uuid>>;
	async fn cached(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>>;
	async fn cache(&mut self, key: Uuid, digest: &str, result: &Value) -> Result<()>;
	/// Replace a pending Scratch patch claim while holding the request lock.
	async fn complete_patch(
		&mut self,
		_key: Uuid,
		_digest: &str,
		_claim: Uuid,
		_result: &Value,
	) -> Result<()> {
		Err(crate::Error::External(
			"Scratch patch settlement unavailable".into(),
		))
	}
}
