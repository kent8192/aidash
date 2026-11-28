//! Native file services adapt unchanged DTOs to shared application file workflows.
use super::contracts::*;
use crate::{
	Result, authorization::access::Access, domain::Run, registry::AgentConfig, store::Store,
};
use aidash_application::capabilities::files as application;
use serde_json::Value;
pub(crate) async fn settings(access: &mut Access, run: &Run) -> Result<AgentConfig> {
	application::settings(
		&mut crate::bootstrap::file_scope(None, access, Some(run)),
		&run.metadata(),
	)
	.await
	.map_err(Into::into)
}
pub(crate) enum Invocation {
	Complete(Box<Envelope>),
	Pod(Box<application::DeferredPodFile>),
}

pub(crate) async fn invoke(
	store: &Store,
	access: &mut Access,
	run: &Run,
	name: &str,
	input: Value,
	key: &str,
) -> Result<Invocation> {
	match application::invoke(
		&mut crate::bootstrap::file_scope(Some(store), access, Some(run)),
		&run.metadata(),
		name,
		input,
		key,
	)
	.await?
	{
		application::Invocation::Complete(output) => {
			Ok(Invocation::Complete(Box::new(envelope(output)?)))
		}
		application::Invocation::Pod(request) => Ok(Invocation::Pod(request)),
	}
}

pub(crate) fn envelope(output: aidash_domain::capabilities::files::Envelope) -> Result<Envelope> {
	Ok(Envelope {
		operation_id: output.operation_id,
		status: output.status,
		policy_revision: output.policy_revision,
		area_id: output.area_id,
		generation: output.generation,
		revision: output.revision,
		result: serde_json::from_value(output.result)?,
	})
}
