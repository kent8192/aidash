//! A Tenant's Retention Rules, read and changed in one transaction (ADR 0045).
use crate::Result;
use aidash_domain::retention::{Decider, RetentionRules};
use async_trait::async_trait;

#[async_trait]
pub trait RetentionScope: Send {
	/// Who is reading or changing the periods.
	fn decider(&self) -> Decider;
	/// The Deployment's periods, which are also the longest a Tenant may keep.
	fn deployment(&self) -> RetentionRules;
	/// A subject must be permitted `action` on its Tenant's `retention`
	/// collection; a Deployment Administrator needs no Tenant policy.
	async fn require(&mut self, tenant: &str, action: &str) -> Result<()>;
	/// The Tenant's stored periods, locked until the scope finishes.
	async fn stored(&mut self, tenant: &str) -> Result<Option<RetentionRules>>;
	/// Store or clear the Tenant's periods and keep the change in history.
	async fn store(
		&mut self,
		tenant: &str,
		rules: Option<&RetentionRules>,
		decider: &Decider,
	) -> Result<()>;
	async fn finish(self: Box<Self>, committed: bool) -> Result<()>;
}

#[async_trait]
pub trait RetentionRepository: Send + Sync {
	async fn begin(&self) -> Result<Box<dyn RetentionScope + '_>>;
}
