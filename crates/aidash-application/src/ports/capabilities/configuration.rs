//! New immutable versions, provenance and historical text attachments commit together.
use crate::Result;
use aidash_domain::{
	capabilities::records::Record,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
pub struct Limits {
	pub files: usize,
	pub bytes: u64,
	pub text_bytes: usize,
}
#[async_trait]
pub trait ConfigurationScope: Send {
	fn principal(&self) -> &str;
	fn limits(&self) -> Limits;
	async fn entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry>;
	async fn cached(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>>;
	async fn cache(&mut self, key: Uuid, digest: &str, result: &Value) -> Result<()>;
	async fn reference(&mut self, id: Uuid) -> Result<Record>;
	async fn bindings(
		&mut self,
		entry: &Entry,
	) -> Result<aidash_domain::registry::bindings::BindingSnapshot>;
	async fn register(&mut self, entry: &Entry) -> Result<bool>;
	async fn provenance(&mut self, source: &EntityRef, entry: &Entry) -> Result<()>;
	async fn event(&mut self, kind: &str, data: Value) -> Result<()>;
}
