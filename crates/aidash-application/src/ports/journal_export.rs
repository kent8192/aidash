//! Authorized access to the resolved Context Journal.
use crate::{Result, tenancy::TenancyScope};
use aidash_domain::context::journal_export::JournalLine;
use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
pub trait JournalSource: Send + Sync {
	/// Export an inclusive sequence range under the reader Run's authority.
	async fn export(
		&self,
		scope: &TenancyScope,
		reader_run: Uuid,
		target_run: Uuid,
		range: Option<(u64, u64)>,
	) -> Result<Vec<JournalLine>>;
}
