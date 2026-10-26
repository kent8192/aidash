//! Identity requests cancellation through Execution's service boundary. The
//! caller's transaction retains atomic deletion and cancellation admission.
use crate::{Result, federation::Federation};
use reinhardt::db::backends::TransactionExecutor;
use uuid::Uuid;

pub(crate) async fn record_deletion_in(
	runtime: &Federation,
	tx: &mut dyn TransactionExecutor,
	deletion: Uuid,
	tenant: &str,
	node: &str,
) -> Result<()> {
	super::super::repositories::node_cancellations::record(runtime, tx, deletion, tenant, node)
		.await
}
