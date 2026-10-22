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
	/// How a test endpoint may connect: public addresses only, unless the
	/// Deployment permits its origin (#361, ADR 0036).
	fn destination(&self, endpoint: &str) -> Result<aidash_domain::keyless::Destination>;
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
	/// The same page for those who write the profiles. A Subject needs
	/// `workbench.test_profile.update` in the same transaction.
	async fn administered(&self, tenant: &str) -> Result<Vec<TestProfile>>;
	/// Decides `workbench.test_profile.update` for a Subject before anything
	/// about the rules is read, keeping nothing; `save` decides it again with
	/// the write, which keeps that decision.
	async fn authorize_update(&self, tenant: &str, id: &str) -> Result<()>;
	/// The hosts the Tenant's active Tenant Secret `name` records, or `None`
	/// when there is none. A Subject needs `secret.use` on it; a placement
	/// without a Secret Store holds none.
	async fn tenant_secret_hosts(&self, tenant: &str, name: &str) -> Result<Option<Vec<String>>> {
		let _ = (tenant, name);
		Ok(None)
	}
	/// The definition in the profile's Tenant, which the caller names.
	async fn definition(&self, tenant: &str, reference: &EntityRef) -> Result<Entry>;
	/// Preserve the exclusive row lock, expected-revision fence and atomic
	/// update/reload. A Subject needs `workbench.test_profile.update` in the
	/// same transaction.
	async fn save(&self, tenant: &str, id: &str, input: &ProfileInput) -> Result<TestProfile>;
}
