//! Live admission and terminal outcomes are shared by background and explicit delivery.
use crate::{Error, Result, ports::execution::terminal::*};
use aidash_domain::{RunControl, TaskStatus};

/// Receiver cancellation after revocation must not require reaching its Home node.
pub async fn finish_cancelled(scope: &mut dyn FailureScope) -> Result<bool> {
	if scope.metadata().control == RunControl::Cancelled && scope.remote_grant().await? {
		scope.finish(Ok(TaskStatus::Cancelled)).await?;
		return Ok(true);
	}
	Ok(false)
}

pub async fn deliver(scope: &mut dyn FailureScope) -> Result<TaskStatus> {
	scope.admit().await?;
	let result = async {
		scope.deliver_inputs().await?;
		let task = scope.task().await?;
		if task.status.is_terminal() {
			return Ok(task.status);
		}
		Ok(scope.transition(scope.target()).await?.status)
	}
	.await;
	scope.finish_authority(result).await
}

pub async fn settle(scope: &mut dyn FailureScope, result: Result<TaskStatus>) -> Result<()> {
	if let Err(
		error @ (Error::Forbidden | Error::Unauthorized | Error::IdentityStatusUnavailable),
	) = &result
	{
		scope
			.pause_authority(matches!(error, Error::IdentityStatusUnavailable))
			.await?;
		return Ok(());
	}
	scope.finish(result).await
}

/// A failed input delivery is deferred without starving ordinary runnable work.
pub async fn pending(repository: &dyn TerminalRepository) -> Result<bool> {
	let Some(run) = repository.pending_inputs().await? else {
		return Ok(false);
	};
	match repository.deliver_inputs(&run).await {
		Ok(()) => Ok(true),
		Err(error) => {
			tracing::warn!(run_id=%run.id, %error, "terminal run message delivery deferred");
			repository.defer_inputs(run.id).await?;
			Ok(false)
		}
	}
}

#[cfg(test)]
mod tests;
