//! Remote compaction releases local authority before bounded allowance RPCs.
use crate::{
	Result,
	ports::{CompactionQuestions, catalog::CatalogScope},
};
use aidash_domain::{Run, generation::dispatch::Input, registry::CompactorConfig};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
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

#[async_trait]
pub trait RemoteCompactionTransport: Send + Sync {
	fn check_credential(&self) -> Result<()>;
	fn check_request(&self, state: &Value, questions: &CompactionQuestions) -> Result<usize>;
	async fn ask(&self, state: &Value, questions: &CompactionQuestions) -> Result<Value>;
}
pub trait RemoteCompactionProvider: Send + Sync {
	fn approved_remote(
		&self,
		config: CompactorConfig,
	) -> Result<Arc<dyn RemoteCompactionTransport>>;
}
