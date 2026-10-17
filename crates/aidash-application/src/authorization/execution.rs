//! HTTP dispatch, worker steps, and recovery retain the same execution authority.
use crate::{
	Error, Result,
	ports::authorization::{ExecutionGrantRepository, ExecutionGrantSession},
};
use aidash_domain::{
	RunMetadata,
	identity::execution::ExecutionGrant,
	policy::{PolicyBundle, SubjectKind},
};
use uuid::Uuid;

pub async fn grant(
	repository: &dyn ExecutionGrantRepository,
	run: &RunMetadata,
) -> Result<Option<ExecutionGrant>> {
	let grant = repository.grant(run.id).await?;
	if let Some(grant) = &grant {
		if !grant.matches_worker(repository.node_id(), run) {
			return Err(Error::Forbidden);
		}
	} else {
		repository
			.require_legacy_remote_task(&run.home_node, run.task_id)
			.await?;
		repository
			.require_legacy_execution(run.workspace_id)
			.await?;
		repository
			.require_legacy_agent(&run.agent_id, &run.agent_version)
			.await?;
	}
	Ok(grant)
}

async fn workspace_read(scope: &mut dyn ExecutionGrantSession, run: &RunMetadata) -> Result<()> {
	let workspace = scope.workspace(run.workspace_id).await?;
	scope.set_context(workspace.attributes.clone());
	scope.require(&workspace, "workspace.read").await
}

pub async fn bind_worker(
	scope: &mut dyn ExecutionGrantSession,
	run: &RunMetadata,
	initial: &ExecutionGrant,
	durable_audit: bool,
) -> Result<()> {
	let current = scope.required_grant(run.id).await?;
	if !current.same_source(initial) {
		return Err(Error::External(
			"execution authority changed; retry boundary".into(),
		));
	}
	scope.set_subjects(initial.subject_chain.clone());
	scope.select_worker(run.id, Some(durable_audit));
	workspace_read(scope, run).await
}

pub async fn inherit_run(scope: &mut dyn ExecutionGrantSession, run: &RunMetadata) -> Result<()> {
	let grant = scope
		.optional_grant(run.id)
		.await?
		.ok_or(Error::Forbidden)?;
	if !grant.matches_inherited(&scope.identity(), run) {
		return Err(Error::NotFound("run unavailable".into()));
	}
	scope.set_subjects(grant.subject_chain);
	Ok(())
}

pub async fn refresh_worker(
	scope: &mut dyn ExecutionGrantSession,
	node: &str,
	run: &RunMetadata,
) -> Result<()> {
	scope.refresh(run.id).await?;
	let current = scope
		.optional_grant(run.id)
		.await?
		.ok_or(Error::Forbidden)?;
	if !current.matches_refreshed(node, &scope.identity(), run) {
		return Err(Error::External(
			"execution authority changed; retry boundary".into(),
		));
	}
	scope.set_subjects(current.subject_chain);
	scope.select_worker(run.id, None);
	workspace_read(scope, run).await
}

/// Where a task's delegation chain came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inherited {
	/// The task has no origin; the admitting subject starts the chain.
	None,
	/// A Run on this node created the task.
	Local,
	/// The task's chain arrived through Federation: the task is the output of
	/// a remote grant, or a local Run descended from one created it. A local
	/// chain too long to walk to its start is treated the same way.
	Remote,
}

pub async fn inherit_task(scope: &mut dyn ExecutionGrantSession, task: Uuid) -> Result<bool> {
	Ok(inherit(scope, task).await? != Inherited::None)
}

/// Adopts the task's origin chain, as [`inherit_task`], and reports where it
/// came from.
pub async fn inherit(scope: &mut dyn ExecutionGrantSession, task: Uuid) -> Result<Inherited> {
	let (origin, inherited) = match scope.local_origin(task).await? {
		// A local task keeps the Federation provenance of the chain that
		// created it, or one local hop would shed it.
		Some(origin) if scope.federated_origin(task).await? => (Some(origin), Inherited::Remote),
		Some(origin) => (Some(origin), Inherited::Local),
		None => (scope.remote_origin(task).await?, Inherited::Remote),
	};
	if let Some(origin) = origin {
		if !origin.compatible(&scope.identity(), scope.subjects()) {
			return Err(Error::Forbidden);
		}
		scope.set_subjects(origin.subject_chain);
		Ok(inherited)
	} else {
		Ok(Inherited::None)
	}
}

pub fn require_agent(bundle: &PolicyBundle, id: &str) -> Result<()> {
	if bundle
		.subjects
		.get(id)
		.is_none_or(|subject| subject.kind != SubjectKind::Agent)
	{
		Err(Error::Forbidden)
	} else {
		Ok(())
	}
}

#[cfg(test)]
mod tests;

pub mod admission;

pub mod guard;
