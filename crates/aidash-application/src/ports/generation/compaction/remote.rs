//! Remote compaction releases local authority before bounded allowance RPCs.
use crate::{Result, ports::catalog::CatalogScope};
use aidash_domain::{Run, generation::dispatch::Input};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

#[async_trait]
pub trait RemoteCompactionAuthority: Send {
	fn node_id(&self) -> &str;
	fn catalog(&mut self) -> &mut dyn CatalogScope;
	async fn suspend(&mut self) -> Result<()>;
	async fn refresh(&mut self, run: &Run) -> Result<bool>;
	async fn description(&mut self, run: Uuid) -> Result<Value>;
	/// The local authority lease is suspended before Home allowance admission.
	async fn admit(
		&mut self,
		run: &Run,
		attempt: Uuid,
		digest: String,
		amount: i64,
	) -> Result<Input>;
	/// Persist and deliver Settled with no reported usage, retaining the charge.
	async fn settle(&mut self, input: &Input) -> Result<()>;
}
