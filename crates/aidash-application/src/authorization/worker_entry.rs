//! Worker entry prefers a scoped receiver and otherwise authorizes the current
//! local Run. A Run with neither belongs to no Tenant and never executes.
use crate::Error;
use crate::{Result, ports::authorization::worker_entry::WorkerEntryRepository};
use aidash_domain::{Run, RunControl, RunMetadata, registry::AgentConfig};

/// A Scratch Space authority denial is a named, fail-closed worker refusal.
pub fn scratch_refusal(error: Error) -> Error {
	match error {
		Error::Forbidden
		| Error::Unauthorized
		| Error::NotFound(_)
		| Error::Domain(aidash_domain::Error::Invalid(_)) => {
			Error::AuthorityRefused(Box::new(aidash_domain::tool::AuthorityRefusal {
				tool: "scratch_space".into(),
				check: aidash_domain::tool::RefusalCheck::Policy,
				action: Some("scratch.use".into()),
				subject: None,
				call: None,
			}))
		}
		other => other,
	}
}
pub struct WorkerEntry<S> {
	pub scope: S,
	pub agent: AgentConfig,
	pub remote: bool,
	pub scratch_space_id: Option<uuid::Uuid>,
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
			scratch_space_id: None,
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
		scratch_space_id: None,
	})
}
pub async fn execution<S: Send>(
	repository: &dyn WorkerEntryRepository<S>,
	run: &Run,
) -> Result<WorkerEntry<S>> {
	let mut entry = acquire(repository, &run.metadata(), true).await?;
	entry.scratch_space_id = Some(
		repository
			.scratch_space(&mut entry.scope, run)
			.await
			.map_err(scratch_refusal)?,
	);
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
