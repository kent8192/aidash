//! The Home's side of a delivery from a Run executing for another Home (ADR 0139, decision 20).
//!
//! A delivery is bound to a Remote Grant and the admission that grant started, exactly as a Home
//! command is. The Home checks that binding, the tool of the Run's Binding that the Run
//! called, and the task the grant belongs to, before it stages or stores anything. Where
//! the bytes go is never in the request: the Home takes the Working Area of the Run that
//! delegated, or the Workspace of the grant's task, from its own records.
use crate::{Error, Result, ports::authorization::commands::RemoteCommandScope};
use aidash_domain::{
	Task, TaskStatus,
	federation::delivery::{File, Limits, Purpose, validate},
};
use uuid::Uuid;

/// Who a delivery names, as the Home's inbound authentication and the grant's inspection give it.
pub struct Identity<'a> {
	pub grant_id: Uuid,
	pub admission_id: Uuid,
	/// The receiving Node that sent the request.
	pub node: &'a str,
	/// The Agent version the grant admitted.
	pub agent: &'a str,
	pub version: &'a str,
}

/// Admit one request of a delivery, in the order a Home command is admitted: the live grant's
/// Binding must be this admission's, the Run's Binding must hold the tool the Run called, and
/// the grant's task must still be the executing Agent's and not finished. Returns the task,
/// whose Workspace and delegation the Home's records resolve the target from.
pub async fn verify(
	scope: &mut dyn RemoteCommandScope,
	who: &Identity<'_>,
	tool: &str,
) -> Result<Task> {
	let bound = scope.binding(who.grant_id).await?.ok_or(Error::Forbidden)?;
	if bound.admission_id != who.admission_id {
		return Err(Error::Forbidden);
	}
	scope.require_builtin(tool).await?;
	let task = scope.task(bound.task_id).await?;
	let owner = aidash_domain::agent_subject(who.node, who.agent, who.version);
	if task.owner.as_deref().is_some_and(|saved| saved != owner) {
		return Err(Error::Forbidden);
	}
	if matches!(
		task.status,
		TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled | TaskStatus::Abandoned
	) {
		return Err(Error::Conflict("remote task is terminal".into()));
	}
	Ok(task)
}

/// A delivery's manifest against the Home's budgets. The receiving Node checked the same before
/// it sent anything; the Home never relies on that.
pub fn check_manifest(purpose: &Purpose, files: &[File], share: Limits) -> Result<u64> {
	validate(purpose, files, share).map_err(Into::into)
}

/// Which Node a Run executing for another Home may send its files to: the Home that started it
/// and no other (ADR 0114 amendment). `recipient` is the Node the tool call names.
pub fn require_home(home_node: &str, recipient: &str) -> Result<()> {
	if home_node == recipient {
		Ok(())
	} else {
		Err(Error::Forbidden)
	}
}
#[cfg(test)]
mod tests;
