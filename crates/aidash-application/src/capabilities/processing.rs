//! Reconciliation batches keep isolated receipts retryable without repeating execution.
use crate::{Error, Result, ports::capabilities::processing::OperationProcessingRepository};
use aidash_domain::capabilities::operations::processing::{journal_lost, storage_blockage};
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Default)]
pub struct ReceiptCursor {
	operation: Uuid,
	allocation: super::pod_usage::Cursor,
}
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
	if let Err(error) = super::pod_usage::sweep(repository, &mut cursor.allocation).await {
		tracing::warn!(%error, "allocation usage delivery pending");
	}
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
	let rows = repository.receipts(cursor.operation).await?;
	if rows.is_empty() {
		cursor.operation = Uuid::nil();
	}
	for receipt in rows {
		cursor.operation = receipt.id;
		let health = repository.runner_request("GET", "/v1/health", None).await?;
		let lost = journal_lost(receipt.runner_instance.as_deref(), &health);
		let mut ack = json!({"digest":receipt.digest});
		let mut uncertain = receipt.uncertain;
		if !uncertain && !lost {
			// A writer that Node cancellation withdrew may have reached the Runner
			// as a stopped `uncertain` outcome. Its ACK then needs the adopted
			// output proof, so the Runner's own outcome selects the protocol.
			let path = format!("/v1/operations/{}/ack", receipt.id);
			if repository
				.runner_request("POST", &path, Some(ack.clone()))
				.await
				.is_ok()
			{
				repository.mark_acknowledged(receipt.id).await?;
				continue;
			}
			let Ok(observed) = repository
				.runner_request("GET", &format!("/v1/operations/{}", receipt.id), None)
				.await
			else {
				continue;
			};
			if observed["operation_id"] != json!(receipt.id)
				|| observed["digest"] != receipt.digest
				|| observed["status"] != "uncertain"
				|| observed["termination_confirmed"] != true
			{
				continue;
			}
			uncertain = true;
		}
		if uncertain {
			// Journal loss cannot prove adoption of an uncertain result.
			if lost {
				continue;
			}
			match super::receipt_output::adopt(repository, &receipt).await {
				Ok(Some(proof)) => ack["output_digest"] = json!(proof),
				Ok(None) => {
					// This request only harvests, and deliberately cannot drain a
					// newly recovered snapshot before server adoption.
					let _ = repository
						.runner_request(
							"POST",
							&format!("/v1/operations/{}/ack", receipt.id),
							Some(ack),
						)
						.await;
					continue;
				}
				Err(error) => {
					tracing::warn!(id=%receipt.id,%error,"stopped output adoption pending");
					continue;
				}
			}
		}
		if !lost {
			let response = repository
				.runner_request(
					"POST",
					&format!("/v1/operations/{}/ack", receipt.id),
					Some(ack.clone()),
				)
				.await;
			let Ok(response) = response else {
				continue;
			};
			if uncertain {
				if response["operation_id"] != json!(receipt.id)
					|| response["digest"] != receipt.digest
					|| response["status"] != "uncertain"
					|| response["termination_confirmed"] != true
					|| response["acknowledged"] != true
					|| response["acknowledged_output_digest"] != ack["output_digest"]
				{
					continue;
				}
				let health = repository.runner_request("GET", "/v1/health", None).await?;
				if health["instance"].as_str() != receipt.runner_instance.as_deref() {
					continue;
				}
			}
		}
		repository.mark_acknowledged(receipt.id).await?;
	}
	Ok(())
}
#[cfg(test)]
mod tests;
