//! Read adapters retain all authority checks; declarations only select behavior.
use crate::Result;
use aidash_domain::context::source::{DetailRequest, SourceBatch, SourceRequest};
use async_trait::async_trait;

#[async_trait]
pub trait ContextSourceProvider: Send + Sync {
	async fn fetch(&self, request: &SourceRequest) -> Result<SourceBatch>;
	async fn detail(&self, request: &DetailRequest) -> Result<SourceBatch>;
}
