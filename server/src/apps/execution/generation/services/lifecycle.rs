//! Existing worker callers adapt their transaction to portable lifecycle decisions.
use super::Request;
pub(crate) use crate::apps::execution::generation::repositories::lifecycle::load;
pub use crate::apps::execution::generation::serializers::lifecycle::{Action, Control, History};
use crate::{Result, federation::Federation};

pub(crate) async fn transition(
	f: &Federation,
	tx: &mut crate::database::native::Transaction,
	job: &Request,
	status: &str,
	actor: &str,
	reason: &str,
) -> Result<Request> {
	aidash_application::generation::lifecycle::transition(
		&mut crate::bootstrap::generation_lifecycle_scope(f, tx),
		job,
		status,
		actor,
		reason,
	)
	.await
	.map_err(Into::into)
}
