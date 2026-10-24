//! Admission keeps concrete local and Home reservations outside the use case.
use crate::Result;
use async_trait::async_trait;
use uuid::Uuid;
#[async_trait]
pub trait InferenceAdmissionRepository<R: Send>: Send + Sync {
	fn remote(&self) -> bool;
	async fn suspend(&self) -> Result<()>;
	async fn admit_remote(&self, attempt: Uuid, digest: String, units: i64) -> Result<R>;
	async fn reserve_local(&self, attempt: Uuid, window: usize, output: u32) -> Result<Option<R>>;
}
