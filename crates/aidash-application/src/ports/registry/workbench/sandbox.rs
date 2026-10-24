//! Sandbox management shares current draft authority and the existing admission/stop transactions.
use crate::Result;
use aidash_domain::{
	identity::Principal,
	registry::workbench::{
		Draft,
		sandbox::{TestLimitCeilings, TestLimits, TestSession},
	},
};
use async_trait::async_trait;
use uuid::Uuid;
#[async_trait]
pub trait SandboxScope: Send {
	async fn draft(&mut self, id: Uuid, lock: bool) -> Result<Draft>;
	async fn authorize_draft(&mut self, draft: &Draft, action: &str, shares: bool) -> Result<()>;
	/// Requires a Tenant action of a Subject of `tenant` on resource `id` in
	/// this transaction, auditing a denial. A Deployment Administrator passes.
	async fn authorize_tenant(&mut self, tenant: &str, action: &str, id: &str) -> Result<()>;
	/// The limits that apply, within the Deployment's ceilings. Preserve the
	/// tenant limits UPDATE lock, including creation of missing defaults.
	async fn limits(&mut self, tenant: &str) -> Result<TestLimits>;
	/// The limits as the Tenant set them, under the same lock.
	async fn stored_limits(&mut self, tenant: &str) -> Result<TestLimits>;
	/// Writes limits read at `limits.revision`; returns them at the new one.
	async fn save_limits(&mut self, limits: &TestLimits) -> Result<TestLimits>;
	async fn session(&mut self, id: Uuid, lock: bool) -> Result<TestSession>;
	async fn sessions(&mut self, draft: Uuid) -> Result<Vec<TestSession>>;
	/// Preserve status=running and active-slot release in one atomic update/reload.
	async fn stop(&mut self, id: Uuid) -> Result<TestSession>;
	async fn commit(self: Box<Self>) -> Result<()>;
}
#[async_trait]
pub trait SandboxRepository: Send + Sync {
	fn principal(&self) -> Principal;
	/// The Deployment's ceilings on Tenants' Test Limits.
	fn ceilings(&self) -> TestLimitCeilings;
	/// The HTTP adapter retains its existing field-validation diagnostics.
	fn validate_limit_fields(&self, limits: &TestLimits) -> Result<()>;
	async fn begin(&self) -> Result<Box<dyn SandboxScope + '_>>;
	/// This is the original atomic payload purge and abandoned-active-slot cleanup.
	async fn purge(&self) -> Result<u64>;
}

pub mod dispatch;

pub mod execution;

pub mod admission;
