//! Profile reads keep draft-test authority separate from the existing profile transaction.
use crate::Result;
use aidash_domain::{
	identity::Principal,
	registry::{
		EntityRef, Entry,
		workbench::{
			Draft,
			profile::{ProfileInput, TestProfile},
		},
	},
};
use async_trait::async_trait;
use uuid::Uuid;
pub trait ProfileConfiguration: Send + Sync {
	fn require_secret(&self, name: &str) -> Result<()>;
}
#[async_trait]
pub trait ProfileDraftScope: Send {
	async fn bindings(
		&mut self,
		entry: &aidash_domain::registry::Entry,
	) -> Result<aidash_domain::registry::bindings::BindingSnapshot>;
	async fn draft(&mut self, id: Uuid) -> Result<Draft>;
	async fn authorize(&mut self, draft: &Draft, action: &str, shares: bool) -> Result<()>;
	async fn commit(self: Box<Self>) -> Result<()>;
}
#[async_trait]
pub trait ProfileRepository: Send + Sync {
	fn principal(&self) -> Principal;
	async fn begin_draft(&self) -> Result<Box<dyn ProfileDraftScope + '_>>;
	/// Preserve the existing ordered 100-row native profile transaction.
	async fn page(&self, tenant: &str) -> Result<Vec<TestProfile>>;
	async fn definition(&self, reference: &EntityRef) -> Result<Entry>;
	/// Preserve the exclusive row lock, expected-revision fence and atomic update/reload.
	async fn save(&self, tenant: &str, id: &str, input: &ProfileInput) -> Result<TestProfile>;
}
