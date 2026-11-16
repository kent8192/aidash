//! Inspection discloses persisted diagnostics only after the current Run visibility decision.
use crate::{
	Error, Result,
	ports::authorization::run_details::{RunDetails, RunDetailsRepository, RunDetailsScope},
};
use aidash_domain::RunInspection;
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;
pub async fn inspect(
	repository: &dyn RunDetailsRepository,
	id: Uuid,
	offset: u64,
) -> Result<RunDetails> {
	let mut scope = repository.begin().await?;
	let result = inspect_in(scope.as_mut(), id, offset).await;
	scope.finish(result).await
}
/// The same visibility decision as [`inspect`], without reading the Run's
/// persisted diagnostics. A missing or invisible Run is `Forbidden`.
pub async fn visible(repository: &dyn RunDetailsRepository, id: Uuid) -> Result<RunInspection> {
	let mut scope = repository.begin().await?;
	let result = visible_in(scope.as_mut(), id).await;
	scope.finish_visible(result).await
}
pub async fn visible_in(scope: &mut dyn RunDetailsScope, id: Uuid) -> Result<RunInspection> {
	let run = in_workspace(scope, id).await?;
	scope.prepare_visible(std::slice::from_ref(&run)).await?;
	if !scope.run_visible(&run).await? {
		return Err(Error::Forbidden);
	}
	Ok(run)
}
/// [`visible_in`] for many Runs, each `None` where [`visible_in`] is `Forbidden`.
///
/// The Runs are read together, and each Workspace's Runs are prepared together
/// under that Workspace's authority, so the reads do not grow with the Runs of
/// one Workspace. Runs of another Node keep the caller's authority and are
/// decided first, before any Workspace context is set; the caller restores its
/// own context afterwards.
pub async fn visible_many_in(
	scope: &mut dyn RunDetailsScope,
	ids: &[Uuid],
) -> Result<HashMap<Uuid, Option<RunInspection>>> {
	let mut visible: HashMap<_, _> = ids.iter().map(|id| (*id, None)).collect();
	let mut remote = Vec::new();
	let mut local = BTreeMap::<Uuid, Vec<RunInspection>>::new();
	for raw in scope.runs(ids).await? {
		if !visible.contains_key(&raw.metadata.id) {
			continue;
		}
		let run = raw.inspect();
		if run.is_local(scope.node_id()) {
			local.entry(run.workspace_id).or_default().push(run);
		} else {
			remote.push(run);
		}
	}
	decide_prepared(scope, remote, &mut visible).await?;
	for (workspace_id, runs) in local {
		let workspace = match scope.workspace(workspace_id).await {
			Ok(workspace) => workspace,
			Err(Error::Forbidden) => continue,
			Err(error) => return Err(error),
		};
		scope.set_context(workspace.attributes.clone());
		match scope.require(&workspace, "workspace.read").await {
			Ok(()) => {}
			Err(Error::Forbidden) => continue,
			Err(error) => return Err(error),
		}
		decide_prepared(scope, runs, &mut visible).await?;
	}
	Ok(visible)
}
/// Prepare `runs` under the current authority and decide each one.
async fn decide_prepared(
	scope: &mut dyn RunDetailsScope,
	runs: Vec<RunInspection>,
	visible: &mut HashMap<Uuid, Option<RunInspection>>,
) -> Result<()> {
	if runs.is_empty() {
		return Ok(());
	}
	match scope.prepare_visible(&runs).await {
		Ok(()) => {}
		// The batch read refused as a whole: decide each Run on its own.
		Err(Error::Forbidden) => {
			for run in runs {
				let id = run.id;
				match scope.prepare_visible(std::slice::from_ref(&run)).await {
					Ok(()) => {}
					Err(Error::Forbidden) => continue,
					Err(error) => return Err(error),
				}
				if decide(scope, &run).await? {
					visible.insert(id, Some(run));
				}
			}
			return Ok(());
		}
		Err(error) => return Err(error),
	}
	for run in runs {
		if decide(scope, &run).await? {
			visible.insert(run.id, Some(run));
		}
	}
	Ok(())
}
async fn decide(scope: &mut dyn RunDetailsScope, run: &RunInspection) -> Result<bool> {
	match scope.run_visible(run).await {
		Ok(visible) => Ok(visible),
		Err(Error::Forbidden) => Ok(false),
		Err(error) => Err(error),
	}
}
async fn in_workspace(scope: &mut dyn RunDetailsScope, id: Uuid) -> Result<RunInspection> {
	let raw = scope.run(id).await?.ok_or(Error::Forbidden)?;
	let run = raw.inspect();
	if run.is_local(scope.node_id()) {
		let workspace = scope.workspace(run.workspace_id).await?;
		scope.set_context(workspace.attributes.clone());
		scope.require(&workspace, "workspace.read").await?;
	}
	Ok(run)
}
pub async fn inspect_in(
	scope: &mut dyn RunDetailsScope,
	id: Uuid,
	offset: u64,
) -> Result<RunDetails> {
	let mut run = in_workspace(scope, id).await?;
	if !scope.run_visible(&run).await? {
		return Err(scope
			.run_reads_block(&run)
			.await?
			.map_or(Error::Forbidden, Error::RunHidden));
	}
	// Offered only to a local caller who may decline it, decided without recording a denial.
	let offered = run.declinable_call.take();
	let declinable_call = match offered {
		Some(offered)
			if run.control == aidash_domain::RunControl::Paused
				&& run.is_local(scope.node_id())
				&& scope.may_decline(&run).await? =>
		{
			Some(offered)
		}
		_ => None,
	};
	let invocations = scope.invocations(id, offset).await?;
	let memory = scope.memory(&run.metadata).await?;
	let media_input_routes = scope.media_input_routes(&run.metadata).await?;
	Ok(RunDetails {
		provider_tool_activity: scope.provider_tool_activity(id, offset).await?,
		egress_requests: scope.egress_requests(id, offset).await?,
		run,
		declinable_call,
		invocations,
		memory,
		media_input_routes,
	})
}
#[cfg(test)]
mod tests;
