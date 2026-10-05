//! Native journal compatibility functions adapt to portable retry and recovery use cases.
use super::{Binding, Operation};
pub(crate) use crate::apps::knowledge::repositories::remote_journal::Record;
use crate::{Result, store::Store};
#[cfg(test)]
pub(crate) use aidash_domain::semantic::remote::journal::retry_delay;
use uuid::Uuid;
pub(crate) async fn prepare(
	store: &Store,
	operation: &Operation,
	binding: &Binding,
) -> Result<Record> {
	aidash_application::semantic::remote_journal::prepare(
		&crate::bootstrap::semantic_journal_repository(store),
		operation,
		binding,
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}
pub(crate) async fn bound(
	store: &Store,
	operation: &Operation,
	binding: &Binding,
) -> Result<Record> {
	aidash_application::semantic::remote_journal::bound(
		&crate::bootstrap::semantic_journal_repository(store),
		operation,
		binding,
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}
pub(crate) async fn resume_in(
	store: &Store,
	tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
	grant: Uuid,
	admission: Uuid,
	workspace: Uuid,
	actor: &str,
) -> Result<()> {
	aidash_application::semantic::remote_journal::resume(
		&mut crate::bootstrap::semantic_journal_scope(store, tx),
		grant,
		admission,
		workspace,
		actor,
	)
	.await
	.map_err(Into::into)
}
#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn retry_schedule_is_bounded_without_resetting_failed_attempts() {
		assert_eq!(
			(1..=6).map(retry_delay).collect::<Vec<_>>(),
			vec![Some(2), Some(4), Some(8), Some(16), Some(32), None]
		);
		assert_eq!(retry_delay(0), None);
		assert_eq!(retry_delay(i32::MAX), None);
	}
}
