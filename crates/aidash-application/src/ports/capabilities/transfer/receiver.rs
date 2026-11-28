//! Scoped staging and receipt settlement for delivered Thread Attachments.
use crate::Result;
use aidash_domain::capabilities::{
	operations::MountedFile,
	records::Record,
	transfer::{Description, receiver::ManifestLimits},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

#[async_trait]
pub trait ReceiverScope: Send {
	fn credential(&self) -> Uuid;
	async fn record(&mut self, id: Uuid) -> Result<Record>;
	async fn insert(&mut self, id: Uuid, data: Value, expires: DateTime<Utc>) -> Result<Record>;
	async fn update(&mut self, record: &mut Record) -> Result<()>;
	async fn reserve(&mut self, bytes: i64) -> Result<()>;
	/// Serialize allocation and quota settlement within the Tenant.
	async fn lock_conversion(&mut self) -> Result<()>;
	async fn put_staging(&mut self, bytes: &[u8]) -> Result<(Uuid, String)>;
	async fn read_file(&mut self, file: &MountedFile) -> Result<Vec<u8>>;
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()>;
}

#[async_trait]
pub trait ReceiverRepository: Send + Sync {
	fn admission_enabled(&self) -> bool;
	fn limits(&self) -> ManifestLimits;
	fn node_id(&self) -> &str;
	fn description(&self) -> &Description;
	async fn begin(&self, source: &str) -> Result<Box<dyn ReceiverScope + '_>>;
}
