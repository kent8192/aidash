//! Read membership must be durable before a tool or provider can retain its output.
use crate::Result;
use async_trait::async_trait;
use uuid::Uuid;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadMembership {
	Run(Uuid),
	RemoteGrant(Uuid),
}
#[async_trait]
pub trait ReadJournalScope: Send {
	fn membership(&self) -> (Option<Uuid>, Option<Uuid>);
	async fn legacy_messages(
		&mut self,
		workspace: Option<Uuid>,
		sender: Option<&str>,
		content: Option<&str>,
	) -> Result<Vec<Uuid>>;
	/// This commit remains independent of the live authority lease.
	async fn record_sources(
		&mut self,
		membership: ReadMembership,
		workspace: Uuid,
		sources: &[(String, Uuid)],
	) -> Result<()>;
	async fn record_registry(
		&mut self,
		run: Uuid,
		ids: Vec<String>,
		versions: Vec<String>,
	) -> Result<()>;
}
#[async_trait]
pub trait GrantJournalScope: Send {
	async fn remote_semantic_sources(&mut self, grant: Uuid) -> Result<()>;
	async fn sources(&mut self, grant: Uuid) -> Result<Vec<(Uuid, String, Uuid)>>;
	async fn source_visible(
		&mut self,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
		pending: &mut Vec<Uuid>,
	) -> Result<bool>;
	async fn run_reads(&mut self, run: Uuid) -> Result<bool>;
}
