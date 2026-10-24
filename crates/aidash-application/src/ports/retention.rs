//! A Tenant's Retention Rules, read and changed in one transaction (ADR 0045).
use crate::Result;
use aidash_domain::retention::{Decider, RetentionChange, RetentionRules};
use async_trait::async_trait;

#[async_trait]
pub trait RetentionScope: Send {
	/// Who is reading or changing the periods.
	fn decider(&self) -> Decider;
	/// The Deployment's periods, which are also the longest a Tenant may keep.
	fn deployment(&self) -> RetentionRules;
	/// Whether a subject is permitted `action` on its Tenant's `retention`
	/// collection; a Deployment Administrator needs no Tenant policy.
	async fn permits(&mut self, tenant: &str, action: &str) -> Result<bool>;
	/// The Tenant's stored periods, locked until the scope finishes.
	async fn stored(&mut self, tenant: &str) -> Result<Option<RetentionRules>>;
	/// Store or clear the Tenant's periods and keep the change in history.
	async fn store(
		&mut self,
		tenant: &str,
		rules: Option<&RetentionRules>,
		decider: &Decider,
	) -> Result<()>;
	/// The Tenant's changes, newest first.
	async fn history(
		&mut self,
		tenant: &str,
		offset: u64,
		limit: u32,
	) -> Result<Vec<RetentionChange>>;
	async fn finish(self: Box<Self>, committed: bool) -> Result<()>;
}

#[async_trait]
pub trait RetentionRepository: Send + Sync {
	async fn begin(&self) -> Result<Box<dyn RetentionScope + '_>>;
}
