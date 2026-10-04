//! Package admission borrows the current run authority and its publication transaction.
use crate::Result;
use aidash_domain::{
	capabilities::{
		operations::{MountedFile, ShellRequest},
		records::Record,
		sessions::Area,
	},
	policy::Resource,
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
pub struct Limits {
	pub outbound_origins: Vec<String>,
	pub package_origins: Vec<String>,
	pub image: Option<String>,
	pub install_seconds: u64,
}
#[async_trait]
pub trait PackageScope: Send {
	fn principal(&self) -> &str;
	fn subjects(&self) -> &[String];
	fn limits(&self) -> Result<Limits>;
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn outbound(&mut self, id: Uuid) -> Result<Record>;
	async fn read(&mut self, file: &MountedFile) -> Result<Vec<u8>>;
	async fn prepare(
		&mut self,
		area: &mut Area,
		input: ShellRequest,
		kind: &str,
		extra: Value,
	) -> Result<Value>;
}
