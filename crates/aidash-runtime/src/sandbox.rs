//! The runtime owns a sandbox session deadline; application classifies and persists the outcome.
use aidash_application::{
	Result,
	registry::workbench::sandbox::execution::{self, Execution, Failure, Job},
};
use std::time::Duration;
/// Runtime drives one admitted session; cancellation drops native authority leases on the future.
pub async fn complete(
	execution: Execution,
	session_id: execution::SessionId,
	job: Job,
) -> Result<()> {
	let result = match tokio::time::timeout(
		Duration::from_secs(job.limits.max_duration_secs as u64),
		execution::simulate(&execution, session_id, &job),
	)
	.await
	{
		Ok(Ok(outcome)) => Ok(outcome),
		Ok(Err(error)) => Err(Failure::Execution(error)),
		Err(_) => Err(Failure::TimedOut),
	};
	execution::settle(execution.repository.as_ref(), session_id, &job, result).await
}
