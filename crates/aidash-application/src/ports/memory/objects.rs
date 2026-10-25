//! Immutable recovery storage; no cloud types cross the application boundary.
use crate::Result;
use async_trait::async_trait;

pub const MAX_OBJECT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Object {
	pub bytes: Vec<u8>,
	pub generation: String,
}
#[derive(Debug, Clone)]
pub struct ObjectName {
	pub name: String,
	pub generation: String,
}
#[derive(Debug, Clone)]
pub struct ObjectPage {
	pub objects: Vec<ObjectName>,
	pub next: Option<String>,
}

#[async_trait]
pub trait RecoveryObjects: Send + Sync {
	async fn get(&self, name: &str, generation: Option<&str>) -> Result<Option<Object>>;
	/// Create once. An identical replay returns the existing generation; a
	/// different value at the same name is a conflict, never an overwrite.
	async fn create(&self, name: &str, bytes: &[u8]) -> Result<String>;
	async fn list(&self, prefix: &str, page: Option<&str>) -> Result<ObjectPage>;
	/// Only body-archive identities receive deletion permission in production.
	async fn delete(&self, name: &str, generation: &str) -> Result<()>;
}
