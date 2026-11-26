//! The Home's side of a delivery from a Run executing for another Home (ADR 0139, decision 20).
//!
//! A delivery is bound to a Remote Grant and the admission that grant started, exactly as a Home
//! command is. The Home checks that binding, the tool of the Run's Binding that the Run
//! called, and the task the grant belongs to, before it stages or stores anything. Where
//! the bytes go is never in the request: the Home takes the Working Area of the Run that
//! delegated, or the Thread of the grant's task, from its own records.
use crate::{Error, Result, ports::authorization::commands::RemoteCommandScope};
use aidash_domain::{
	Task, TaskStatus,
	federation::{
		delivery::{File, Limits, Purpose, validate},
		tool_lists::FederationToolRequest,
	},
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
	/// The Home's current Federation Tool Request for the receiving Node, with this grant's
	/// Run-only answers; `None` for a receiver of the Home's own Tenant (ADR 0139).
	pub request: Option<&'a FederationToolRequest>,
}

/// Admit one request of a delivery, in the order a Home command is admitted: the live grant's
/// Binding must be this admission's, the Home's own Request and the Run's Binding must hold the
/// tool the Run called, and the grant's task must still be the executing Agent's and not
/// finished. Returns the task, whose Thread and delegation the Home's records resolve the
/// target from.
pub async fn verify(
	scope: &mut dyn RemoteCommandScope,
	who: &Identity<'_>,
	tool: &str,
) -> Result<Task> {
	// A Workspace tool a saved Agent pins is its Thread tool here, as on the lists (#1008).
	let tool = aidash_domain::tool::thread_tools::current(tool);
	let bound = scope.binding(who.grant_id).await?.ok_or(Error::Forbidden)?;
	if bound.admission_id != who.admission_id {
		return Err(Error::Forbidden);
	}
	// The Home stores what a delivery carries itself, so its own Request must name the tool the
	// Run called, whatever the receiving Node checked (ADR 0139, decision 1).
	if tool != "completion_attachments" {
		crate::authorization::peer::tool_request::require_requested(who.request, tool)?;
	}
	if tool == "thread_message" {
		// A delivered message always carries files, which the lists name with a key of their own
		// besides the plain one: the default `thread_message` sends no files (decision 6).
		crate::authorization::peer::tool_request::require_requested_key(
			who.request,
			aidash_domain::tool::ToolReach::HomeWorkspace,
			aidash_domain::federation::tool_lists::THREAD_MESSAGE_FILES,
		)?;
	}
	if tool != "completion_attachments" {
		scope.require_builtin(tool).await?;
	}
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
