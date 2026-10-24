//! Preconditions and paired obligations for atomic task and execution completion.
use super::{Mutation, Participant};
use crate::{Error, Result, RunControl, RunMetadata, RunPhase, Task, TaskStatus};
use uuid::Uuid;

pub fn validate_task(task: &Task, revision: i64) -> Result<()> {
	if task.revision != revision || task.status != TaskStatus::Running || task.owner.is_none() {
		return Err(Error::Conflict(
			"task must be running at its expected revision".into(),
		));
	}
	Ok(())
}

/// Lease liveness is measured by storage while holding the execution row lock.
pub fn validate_run(run: &RunMetadata, leased: bool, task: Uuid, revision: i64) -> Result<()> {
	if run.task_id != task
		|| run.revision != revision
		|| run.phase != RunPhase::ToolCall
		|| run.control == RunControl::Cancelled
		|| leased
	{
		return Err(Error::Conflict(
			"run must be quiescent at its expected tool-call revision".into(),
		));
	}
	Ok(())
}

pub fn validate_delegated_completion(participant: &Participant, task: Uuid) -> Result<()> {
	if !participant
		.mutations
		.iter()
		.any(|mutation| matches!(mutation, Mutation::FinishRun { task_id, .. } if *task_id == task))
	{
		return Err(Error::Invalid(
			"delegated completion requires its participant's execution finalization".into(),
		));
	}
	Ok(())
}

pub fn validate_home_completion(participant: &Participant, task: Uuid) -> Result<()> {
	if !participant.mutations.iter().any(
		|mutation| matches!(mutation, Mutation::CompleteTask { task_id, .. } if *task_id == task),
	) {
		return Err(Error::Invalid(
			"execution finalization requires the home task's atomic completion".into(),
		));
	}
	Ok(())
}

#[cfg(test)]
mod tests;
