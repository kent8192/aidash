//! Local inference uses the inherited authority scope and an owned ORM transaction.
use crate::Result;
use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
pub trait GenerationInferenceAuthority: Send {
	/// Return generated ancestors from current subjects, sorted by request ID.
	async fn requests(&mut self, node: &str) -> Result<Vec<Uuid>>;
}
/// Drop rolls back all provisional charges, refunds and usage reports.
#[async_trait]
pub trait GenerationInferenceSession: Send {
	async fn charge(&mut self, request: Uuid, amount: i64) -> Result<()>;
	async fn reserve(&mut self, request: Uuid, attempt: Uuid, run: Uuid, amount: i64)
	-> Result<()>;
	async fn refund(&mut self, request: Uuid, amount: i64) -> Result<()>;
	/// The Tenant and policy of a request whose quota was already released,
	/// with the token limit it was recorded with.
	async fn released_policy(&mut self, request: Uuid) -> Result<Option<(String, String, i64)>>;
	async fn refund_allocated(&mut self, tenant: &str, policy: &str, amount: i64) -> Result<()>;
	async fn report(&mut self, request: Uuid, attempt: Uuid, reported: Option<i64>) -> Result<()>;
	async fn commit(self: Box<Self>) -> Result<()>;
}
#[async_trait]
pub trait GenerationInferenceRepository: Send + Sync {
	fn node_id(&self) -> &str;
	async fn begin(&self) -> Result<Box<dyn GenerationInferenceSession>>;
}
