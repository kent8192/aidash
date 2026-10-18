//! Worker entry prefers a scoped receiver and otherwise authorizes the current
//! local Run. A Run with neither belongs to no Tenant and never executes.
use crate::Error;
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
) -> Result<WorkerEntry<S>> {
	if let Some((scope, agent)) = repository.receiver_lease(run).await? {
		return Ok(WorkerEntry {
			scope,
			agent,
			remote: true,
		});
	}
	let Some(mut scope) = repository.local_lease(run, true).await? else {
		return Err(Error::Forbidden);
	};
	let agent = repository.authorize(&mut scope, run, read_context).await?;
	Ok(WorkerEntry {
		scope,
		agent,
		remote: false,
	})
}
pub async fn execution<S: Send>(
	repository: &dyn WorkerEntryRepository<S>,
	run: &Run,
) -> Result<WorkerEntry<S>> {
	let entry = acquire(repository, &run.metadata(), true).await?;
	if !entry.remote
		&& entry.agent.core_capabilities.enabled()
		&& run.control != RunControl::Cancelled
	{
		repository
			.initialize(&entry.scope, run, &entry.agent)
			.await?;
	}
	Ok(entry)
}
pub async fn delivery<S: Send>(
	repository: &dyn WorkerEntryRepository<S>,
	run: &RunMetadata,
) -> Result<WorkerEntry<S>> {
	acquire(repository, run, false).await
}
#[cfg(test)]
mod tests;
