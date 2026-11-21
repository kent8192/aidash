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
	let run = in_thread(scope, id).await?;
	scope.prepare_visible(std::slice::from_ref(&run)).await?;
	if !scope.run_visible(&run).await? {
		return Err(Error::Forbidden);
	}
	Ok(run)
}
/// [`visible_in`] for many Runs, each `None` where [`visible_in`] is `Forbidden`.
///
/// The Runs are read together, and each Thread's Runs are prepared together
/// under that Thread's authority, so the reads do not grow with the Runs of
/// one Thread. Runs of another Node keep the caller's authority and are
/// decided first, before any Thread context is set; the caller restores its
/// own context afterwards.
pub async fn visible_many_in(
	scope: &mut dyn RunDetailsScope,
	ids: &[Uuid],
) -> Result<HashMap<Uuid, Option<RunInspection>>> {
	visible_many_counted_in(scope, ids)
		.await
		.map(|(visible, _)| visible)
}
/// [`visible_many_in`], and the Node each hidden Run's check gave up on
/// because it did not answer, for a list to count (#770).
pub async fn visible_many_counted_in(
	scope: &mut dyn RunDetailsScope,
	ids: &[Uuid],
) -> Result<(HashMap<Uuid, Option<RunInspection>>, HashMap<Uuid, String>)> {
	let mut silent = HashMap::new();
	let mut visible: HashMap<_, _> = ids.iter().map(|id| (*id, None)).collect();
	let mut remote = Vec::new();
	let mut local = BTreeMap::<Uuid, Vec<RunInspection>>::new();
	for raw in scope.runs(ids).await? {
		if !visible.contains_key(&raw.metadata.id) {
			continue;
		}
		let run = raw.inspect();
		if run.is_local(scope.node_id()) {
			local.entry(run.thread_id).or_default().push(run);
		} else {
			remote.push(run);
		}
	}
	decide_prepared(scope, remote, &mut visible, &mut silent).await?;
	for (thread_id, runs) in local {
		let thread = match scope.thread(thread_id).await {
			Ok(thread) => thread,
			Err(Error::Forbidden) => continue,
			Err(error) => return Err(error),
		};
		scope.set_context(thread.attributes.clone());
		match scope.require(&thread, "thread.read").await {
			Ok(()) => {}
			Err(Error::Forbidden) => continue,
			Err(error) => return Err(error),
		}
		decide_prepared(scope, runs, &mut visible, &mut silent).await?;
	}
	Ok((visible, silent))
}
/// Prepare `runs` under the current authority and decide each one.
async fn decide_prepared(
	scope: &mut dyn RunDetailsScope,
	runs: Vec<RunInspection>,
	visible: &mut HashMap<Uuid, Option<RunInspection>>,
	silent: &mut HashMap<Uuid, String>,
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
				if decide(scope, &run, silent).await? {
					visible.insert(id, Some(run));
				}
			}
			return Ok(());
		}
		Err(error) => return Err(error),
	}
	for run in runs {
		if decide(scope, &run, silent).await? {
			visible.insert(run.id, Some(run));
		}
	}
	Ok(())
}
async fn decide(
	scope: &mut dyn RunDetailsScope,
	run: &RunInspection,
	silent: &mut HashMap<Uuid, String>,
) -> Result<bool> {
	scope.begin_run_check();
	let visible = match scope.run_visible(run).await {
		Ok(visible) => visible,
		Err(Error::Forbidden) => false,
		Err(error) => return Err(error),
	};
	if let (false, Some(node)) = (visible, scope.take_run_silence()) {
		silent.insert(run.id, node);
	}
	Ok(visible)
}
async fn in_thread(scope: &mut dyn RunDetailsScope, id: Uuid) -> Result<RunInspection> {
	let raw = scope.run(id).await?.ok_or(Error::Forbidden)?;
	let run = raw.inspect();
	if run.is_local(scope.node_id()) {
		let thread = scope.thread(run.thread_id).await?;
		scope.set_context(thread.attributes.clone());
		scope.require(&thread, "thread.read").await?;
	}
	Ok(run)
}
pub async fn inspect_in(
	scope: &mut dyn RunDetailsScope,
	id: Uuid,
	offset: u64,
) -> Result<RunDetails> {
	let mut run = in_thread(scope, id).await?;
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
		stints: scope.stints(id).await?,
		run,
		declinable_call,
		invocations,
		memory,
		media_input_routes,
	})
}
#[cfg(test)]
mod tests;
