//! Reconciliation batches keep isolated receipts retryable without repeating execution.
use crate::{Error, Result, ports::capabilities::processing::OperationProcessingRepository};
use aidash_domain::capabilities::operations::processing::{journal_lost, storage_blockage};
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Default)]
pub struct ReceiptCursor(Uuid);
pub fn blocked_publication(error: &Error) -> Option<Value> {
	let message = match error {
		Error::Conflict(message) | Error::Domain(aidash_domain::Error::Conflict(message)) => {
			message
		}
		_ => return None,
	};
	storage_blockage(message)
}
pub async fn sweep(
	repository: &dyn OperationProcessingRepository,
	cursor: &mut ReceiptCursor,
) -> Result<()> {
	let ids = repository.active_operations().await?;
	for id in ids {
		if let Err(error) = Box::pin(super::reconciliation::drive(repository, id)).await {
			if let Some(detail) = blocked_publication(&error) {
				repository.storage_blocked(id, detail).await?;
			}
			tracing::warn!(%id,%error,"capability operation reconciliation pending");
		}
	}
	if let Err(error) = acknowledge(repository, cursor).await {
		tracing::warn!(%error,"runner receipt acknowledgement pending");
	}
	Ok(())
}
pub async fn acknowledge(
	repository: &dyn OperationProcessingRepository,
	cursor: &mut ReceiptCursor,
) -> Result<()> {
	let rows = repository.receipts(cursor.0).await?;
	if rows.is_empty() {
		cursor.0 = Uuid::nil();
	}
	for receipt in rows {
		cursor.0 = receipt.id;
		let health = repository.runner_request("GET", "/v1/health", None).await?;
		let lost = journal_lost(receipt.runner_instance.as_deref(), &health);
		if !lost
			&& repository
				.runner_request(
					"POST",
					&format!("/v1/operations/{}/ack", receipt.id),
					Some(json!({"digest":receipt.digest})),
				)
				.await
				.is_err()
		{
			continue;
		}
		repository.mark_acknowledged(receipt.id).await?;
	}
	Ok(())
}
#[cfg(test)]
mod tests;
