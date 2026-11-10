//! Run-keyed Journal Query session state and termination.
use crate::Result;
use async_trait::async_trait;
use serde::Serialize;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize)]
pub struct Authority {
	pub run: Uuid,
	pub credential: Uuid,
	pub subjects: Vec<String>,
	pub policy_revision: i64,
	pub image: String,
	pub runner_instance: String,
	pub configuration: String,
	pub generation: String,
	pub range: Option<(u64, u64)>,
}

#[derive(Clone, Debug)]
pub struct Session {
	pub id: Uuid,
	pub fingerprint: String,
}

/// Implementations serialize admission for each Run through the entire call.
#[async_trait]
pub trait JournalSessions: Send {
	async fn load(&mut self, run: Uuid) -> Result<Option<Session>>;
	async fn stop(&mut self, session: Uuid) -> Result<bool>;
	async fn save(&mut self, run: Uuid, session: &Session) -> Result<()>;
}
