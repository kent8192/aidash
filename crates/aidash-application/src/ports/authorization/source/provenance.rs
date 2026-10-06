//! Stored disclosures must retain revision, content and current producer permission.
use crate::{Result, authorization::visits::ReadVisit};
use aidash_domain::semantic::{Source, mutations::Entry, remote::SourceRead};
use async_trait::async_trait;
use uuid::Uuid;
#[async_trait]
pub trait SemanticSourceScope: Send {
	fn source_visit(&self, grant: Uuid) -> Option<ReadVisit>;
	/// Read the existing disclosure set in ascending entry ID order.
	async fn disclosed_sources(&mut self, grant: Uuid) -> Result<Vec<SourceRead>>;
	/// Retain a shared entry row lock through permission and content checks.
	async fn disclosed_entry(&mut self, id: Uuid) -> Result<Option<Entry>>;
	async fn source_permitted(&mut self, entry: &Entry) -> Result<bool>;
	async fn source_text(&mut self, workspace: Uuid, source: &Source) -> Result<Option<String>>;
}
