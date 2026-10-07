//! Authorized local claim and delegation use the same atomic scope for HTTP and workers.
use crate::{Error, Result, ports::authorization::admission::ExecutionAdmissionSession};
use aidash_domain::{
	Task, TaskStatus,
	federation::Delegation,
	identity::execution::ExecutionGrant,
	qualified_agent,
	registry::{AgentConfig, EntityRef},
};
use uuid::Uuid;
pub async fn admit(
	scope: &mut dyn ExecutionAdmissionSession,
	task_id: Uuid,
	revision: Option<i64>,
	agent: &EntityRef,
	delegation: bool,
) -> Result<Task> {
	let task = scope.task(task_id).await?.ok_or(Error::Forbidden)?;
	super::inherit_task(scope, task_id).await?;
	let workspace = scope.workspace(task.workspace_id).await?;
	scope.set_context(workspace.attributes.clone());
	let task_resource = scope.task_resource(&task).await?;
	scope.require(&task_resource, "task.read").await?;
	scope.require(&workspace, "workspace.read").await?;
	if task.status != TaskStatus::Open {
		return Err(Error::Conflict("task is already assigned".into()));
	}
	if delegation {
		scope.require(&task_resource, "task.delegate").await?;
	}
	scope.require_live_agent(task_id, agent).await?;
	let subject = qualified_agent(scope.node_id(), &agent.id, &agent.version);
	super::require_agent(scope.bundle(), &subject)?;
	if scope.subjects().len() >= 32 {
		return Err(Error::Invalid(
			"execution delegation depth exceeds 32".into(),
		));
	}
	let mut subjects = scope.subjects().to_vec();
	subjects.push(subject.clone());
	scope.set_subjects(subjects);
	scope.require(&workspace, "workspace.read").await?;
	let entry = scope.executable_entry(agent).await?;
	if !scope.active_installation(&entry).await? {
		return Err(Error::Forbidden);
	}
	if entry.kind != "agent" {
		return Err(Error::Invalid("executor must be an agent".into()));
	}
	scope.check_pinned_installation(&entry).await?;
	scope.require(&task_resource, "task.execute").await?;
	let snapshot = scope.bindings(&entry).await?;
	let config = AgentConfig::from_snapshot(&snapshot)?;
	// The existing thread/tombstone lock precedes claim's event lock.
	let thread = scope.prepare_thread(&task, &config, &agent.id).await?;
	let claimed = scope
		.claim(
			&task,
			revision.unwrap_or(task.revision),
			&subject,
			&entry,
			&snapshot,
		)
		.await?;
	let run_id = scope.claimed_run(task.id).await?;
	let identity = scope.identity();
	let grant = ExecutionGrant {
		run_id,
		task_id: task.id,
		workspace_id: task.workspace_id,
		tenant: identity.tenant,
		credential_id: identity.credential_id,
		root_subject: identity.subject,
		subject_chain: scope.subjects().to_vec(),
	};
	scope.persist_grant(&grant).await?;
	if let Some((identity_id, mapping_id)) = scope.dashboard_origin(grant.credential_id).await? {
		scope
			.persist_dashboard_origin(run_id, identity_id, mapping_id)
			.await?;
	}
	scope
		.admit_thread(&task, run_id, thread, &config, &agent.id)
		.await?;
	Ok(claimed)
}
pub async fn delegate(
	scope: &mut dyn ExecutionAdmissionSession,
	task: Uuid,
	agent: &EntityRef,
) -> Result<Delegation> {
	let admitted = admit(scope, task, None, agent, true).await?;
	let delegation = scope.local_delegation(task, agent).await?;
	scope
		.delegation_event(admitted.workspace_id, &delegation)
		.await?;
	Ok(delegation)
}
#[cfg(test)]
mod tests;
