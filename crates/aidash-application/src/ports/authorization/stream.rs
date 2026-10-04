//! Idle stream snapshots grant no right to emit; each frame retains a live lease.
use crate::Result;
use aidash_domain::Event;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;
pub struct AuthorityRecord {
	pub revision: i64,
	pub document: Value,
	pub owner_subject: Option<String>,
	pub mapping_id: Option<Uuid>,
	pub mapping_enabled: Option<bool>,
	pub issuer: Option<String>,
	pub last_valid_at: Option<DateTime<Utc>>,
	pub disabled_at: Option<DateTime<Utc>>,
}
#[async_trait]
pub trait StreamAuthorityStore: Send + Sync {
	fn tenant(&self) -> &str;
	fn subject(&self) -> &str;
	fn node_id(&self) -> &str;
	fn google_issuer(&self) -> &str;
	fn now(&self) -> DateTime<Utc>;
	/// One unlocked MVCC snapshot; credential expiry remains evaluated by PostgreSQL.
	async fn current(&self, workspace: Option<Uuid>) -> Result<Option<AuthorityRecord>>;
}
#[async_trait]
pub trait StreamSession: Send {
	async fn event_workspaces(&mut self, workspace: Option<Uuid>) -> Result<Vec<Uuid>>;
	async fn require_workspace(&mut self, workspace: Uuid, action: &str) -> Result<()>;
	async fn visible(&mut self, action: &str) -> Result<Vec<Uuid>>;
	async fn allowed(&mut self, workspace: Uuid, action: &str) -> Result<bool>;
	async fn stream_rows(
		&mut self,
		after: i64,
		workspace: Option<Uuid>,
		visible: &[Uuid],
	) -> Result<Vec<Event>>;
	async fn read_rows(
		&mut self,
		cursor: i64,
		workspace: Option<Uuid>,
		visible: &[Uuid],
		page_size: usize,
	) -> Result<Vec<Event>>;
	async fn event_visible(&mut self, event: &Event) -> Result<bool>;
}
