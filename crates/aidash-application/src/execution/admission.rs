//! Provider admission preserves the original accounting mode and remote request identity.
use crate::{Result, ports::execution::admission::InferenceAdmissionRepository};
use aidash_domain::provider::ModelRequest;
use uuid::Uuid;
pub async fn reserve<R: Send>(
	repository: &dyn InferenceAdmissionRepository<R>,
	attempt: Uuid,
	window: usize,
	output: u32,
	request: &ModelRequest,
) -> Result<Option<R>> {
	if repository.remote() {
		repository.suspend().await?;
		let reservation = repository
			.admit_remote(
				attempt,
				request.inference_digest(),
				(window + output as usize) as i64,
				request.response_format.as_ref()
					== Some(&aidash_domain::context::summary::SummaryContent::response_format()),
			)
			.await?;
		return Ok(Some(reservation));
	}
	repository.reserve_local(attempt, window, output).await
}
#[cfg(test)]
mod tests;
