//! Worker entry prefers a scoped receiver and otherwise authorizes the current local Run.
use crate::{Result, ports::authorization::worker_entry::WorkerEntryRepository};
use aidash_domain::{Run, RunControl, RunMetadata, registry::AgentConfig};
pub struct WorkerEntry<S> {
	pub scope: S,
	pub agent: AgentConfig,
	pub remote: bool,
}
async fn acquire<S: Send>(
	repository: &dyn WorkerEntryRepository<S>,
	run: &RunMetadata,
	read_context: bool,
) -> Result<Option<WorkerEntry<S>>> {
	if let Some((scope, agent)) = repository.receiver_lease(run).await? {
		return Ok(Some(WorkerEntry {
			scope,
			agent,
			remote: true,
		}));
	}
	let Some(mut scope) = repository.local_lease(run, true).await? else {
		return Ok(None);
	};
	let agent = repository.authorize(&mut scope, run, read_context).await?;
	Ok(Some(WorkerEntry {
		scope,
		agent,
		remote: false,
	}))
}
pub async fn execution<S: Send>(
	repository: &dyn WorkerEntryRepository<S>,
	run: &Run,
) -> Result<Option<WorkerEntry<S>>> {
	let Some(entry) = acquire(repository, &run.metadata(), true).await? else {
		return Ok(None);
	};
	if !entry.remote
		&& entry.agent.core_capabilities.enabled()
		&& run.control != RunControl::Cancelled
	{
		repository
			.initialize(&entry.scope, run, &entry.agent)
			.await?;
	}
	Ok(Some(entry))
}
pub async fn delivery<S: Send>(
	repository: &dyn WorkerEntryRepository<S>,
	run: &RunMetadata,
) -> Result<Option<WorkerEntry<S>>> {
	acquire(repository, run, false).await
}
#[cfg(test)]
mod tests;
