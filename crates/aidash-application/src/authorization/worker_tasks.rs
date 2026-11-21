//! Worker effects retain their producer, delegated creator and admitted credential.
use crate::authorization::visibility::provenance::ReadBlock;
use crate::{Error, Result, ports::authorization::worker_tasks::WorkerTaskScope};
use aidash_domain::{
	NewTask, RunMetadata, Task,
	federation::Delegation,
	generation::requests::Assignment,
	identity::execution::{CreatedTaskOrigin, TaskOrigin},
	registry::EntityRef,
};
use uuid::Uuid;
/// A missing task and a task the Run may not see read the same, so the model cannot probe for
/// the existence of either.
pub fn task_not_visible(task: Uuid) -> Error {
	Error::Refused(format!(
		"task {task} was not found or is not visible to this Run"
	))
}
/// Why the calling Run may not read a Task it created itself (#1058).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnTaskRefusal {
	/// The `task.read` policy refuses the Task to this Run's authority.
	Policy,
	/// A read the calling Run recorded itself is refused; `None` when the check
	/// that refused did not say which read.
	Read(Option<ReadBlock>),
	/// A read recorded by another Run the Task depends on is refused. The
	/// calling Run never read it, so only its kind is named.
	OtherRunRead(Option<ReadBlock>),
}
fn block_kind(block: &ReadBlock) -> String {
	match block {
		ReadBlock::Source { kind, .. } => kind.clone(),
		ReadBlock::Definition(_) => "definition".into(),
		ReadBlock::RemoteRead => "read from another Node".into(),
		ReadBlock::SemanticRead => "semantic".into(),
	}
}
/// The real reason for a refusal of the Run's own Task. A Run may hear it: the
/// Task is its own, so naming it discloses nothing the Run could probe for. An
/// id is named only for a read the Run recorded itself; a read of another Run
/// the Task depends on is named by kind alone. A Node that did not answer is
/// not a refusal and never reaches this text.
pub fn own_task_refused(task: Uuid, refusal: &OwnTaskRefusal) -> Error {
	let what = match refusal {
		OwnTaskRefusal::Policy => format!("the task.read policy refuses task {task}"),
		OwnTaskRefusal::Read(Some(ReadBlock::Source { kind, id })) => {
			format!("a read it depends on is refused: {kind} {id}")
		}
		OwnTaskRefusal::Read(Some(ReadBlock::Definition(definition))) => format!(
			"a read it depends on is refused: definition {}@{}",
			definition.id, definition.version
		),
		OwnTaskRefusal::Read(Some(block)) => {
			format!("a read it depends on is refused: a {}", block_kind(block))
		}
		OwnTaskRefusal::OtherRunRead(Some(block)) => format!(
			"a {} read by another Run this task depends on is refused",
			block_kind(block)
		),
		OwnTaskRefusal::Read(None) | OwnTaskRefusal::OtherRunRead(None) => {
			"a read it depends on is refused; the check did not say which".into()
		}
	};
	Error::Refused(format!(
		"task {task} was created by this Run, but {what}. Retrying will not change this"
	))
}
async fn subnode_members(
	scope: &mut dyn WorkerTaskScope,
	run: &RunMetadata,
) -> Result<Option<Vec<EntityRef>>> {
	let members = scope.subnode_members(run.thread_id).await?;
	if let Some(members) = &members {
		let caller = EntityRef {
			id: run.agent_id.clone(),
			version: run.agent_version.clone(),
		};
		if !members.contains(&caller) {
			return Err(Error::Forbidden);
		}
	}
	Ok(members)
}

pub async fn assign(
	scope: &mut dyn WorkerTaskScope,
	run: &RunMetadata,
	task: Uuid,
	policy: &str,
	reason: &str,
	draft: Option<&aidash_domain::generation::policy::Draft>,
) -> Result<Assignment> {
	let members = subnode_members(scope, run).await?;
	if scope.task_thread(task).await? != Some(run.thread_id) {
		return Err(task_not_visible(task));
	}
	let assignment = scope
		.assign(task, policy, reason, members.as_deref(), draft)
		.await?;
	scope.record_output(run, "task", task).await?;
	if let Assignment::Generated { generation } = &assignment {
		scope
			.record_output(run, "generation", generation.id)
			.await?;
	}
	Ok(assignment)
}
pub async fn create_task(
	scope: &mut dyn WorkerTaskScope,
	run: &RunMetadata,
	key: &str,
	input: &NewTask,
) -> Result<Task> {
	subnode_members(scope, run).await?;
	let thread = scope.thread(run.thread_id).await?;
	scope.require(&thread, "task.create").await?;
	scope.related_tasks(run.thread_id, input).await?;
	let creator = scope.subjects().last().ok_or(Error::Forbidden)?.clone();
	let task = scope
		.create_task(run.thread_id, input, &creator, key)
		.await?;
	let resource = scope.task_resource(&task).await?;
	scope.require(&resource, "task.read").await?;
	scope.record_output(run, "task", task.id).await?;
	scope.insert_origin(task.id, run.id).await?;
	let actual = scope.created_origin(task.id).await?;
	let identity = scope.identity();
	let expected = CreatedTaskOrigin {
		source_run_id: run.id,
		authority: TaskOrigin {
			tenant: identity.tenant,
			root_subject: identity.subject,
			subject_chain: scope.subjects().to_vec(),
		},
	};
	if actual != expected {
		return Err(Error::Refused(
			"a task from an earlier attempt of this call has a different origin, so it cannot be reused; calling task_create again creates a new task".into(),
		));
	}
	Ok(task)
}
pub async fn delegate(
	scope: &mut dyn WorkerTaskScope,
	run: &RunMetadata,
	task: Uuid,
	node: &str,
	agent: &EntityRef,
) -> Result<Delegation> {
	// A Run delegates on its own Node, which a Tool transport may still name by
	// the bare Deployment id.
	if !run.is_local(scope.node_id()) || !aidash_domain::nodes::names_home(node, &run.home_node) {
		return Err(Error::Forbidden);
	}
	let node = run.home_node.as_str();
	if let Some(members) = subnode_members(scope, run).await?
		&& !members.contains(agent)
	{
		return Err(Error::Refused(
			"Subnode delegation target is not a member".into(),
		));
	}
	if scope.task_thread(task).await? != Some(run.thread_id) {
		return Err(task_not_visible(task));
	}
	// The Run's own Task says why it cannot be read (#1058); any other Task
	// reads the same whether missing or hidden.
	let record = if scope.task_source_run(task).await? == Some(run.id) {
		match scope.own_task_read(task).await {
			Ok(Ok(record)) => record,
			Ok(Err(refusal)) => return Err(own_task_refused(task, &refusal)),
			Err(Error::Forbidden) => return Err(task_not_visible(task)),
			Err(error) => return Err(error),
		}
	} else {
		scope.task_read(task).await.map_err(|error| match error {
			Error::Forbidden => task_not_visible(task),
			error => error,
		})?
	};
	let resource = scope.task_resource(&record).await?;
	scope.require(&resource, "task.delegate").await?;
	if let Some(existing) = scope.execution_grant(task).await? {
		let mut expected = scope.subjects().to_vec();
		expected.push(aidash_domain::agent_subject(
			node,
			&agent.id,
			&agent.version,
		));
		if existing.subject_chain != expected
			|| existing.credential_id != scope.identity().credential_id
		{
			return Err(Error::Refused(
				"the task was already delegated under a different authority".into(),
			));
		}
		scope.record_output(run, "delegated_task", task).await?;
		return Ok(Delegation {
			task_id: task,
			node_id: node.into(),
			agent_id: agent.id.clone(),
			agent_version: agent.version.clone(),
		});
	}
	let delegation = scope.delegate(task, agent).await?;
	scope.record_output(run, "delegated_task", task).await?;
	Ok(delegation)
}
#[cfg(test)]
mod tests;
