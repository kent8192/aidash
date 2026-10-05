//! Native file services adapt unchanged DTOs to shared application file workflows.
use super::contracts::*;
use crate::{
	Result, authorization::access::Access, domain::Run, registry::AgentConfig, store::Store,
};
use aidash_application::capabilities::files as application;
use serde_json::Value;
pub(crate) fn files(area: &Area) -> Result<Vec<FileEntry>> {
	Ok(serde_json::from_value(area.manifest.clone())?)
}
pub(crate) fn available(area: &Area) -> Result<()> {
	aidash_domain::capabilities::operations::available(&area.state).map_err(Into::into)
}
pub(crate) async fn settings(access: &mut Access, run: &Run) -> Result<AgentConfig> {
	application::settings(
		&mut crate::bootstrap::file_scope(None, access, Some(run)),
		&run.metadata(),
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn invoke(
	store: &Store,
	access: &mut Access,
	run: &Run,
	name: &str,
	input: Value,
	key: &str,
) -> Result<Envelope> {
	let output = application::invoke(
		&mut crate::bootstrap::file_scope(Some(store), access, Some(run)),
		&run.metadata(),
		name,
		input,
		key,
	)
	.await?;
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
pub(crate) async fn materialize(
	store: &Store,
	access: &mut Access,
	run: &Run,
	input: Materialize,
) -> Result<Value> {
	application::materialize(
		&mut crate::bootstrap::file_scope(Some(store), access, Some(run)),
		&run.metadata(),
		input.into(),
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn publish(store: &Store, access: &mut Access, area: &mut Area) -> Result<()> {
	let mut state = (&*area).into();
	let result = application::publish(
		&mut crate::bootstrap::file_scope(Some(store), access, None),
		&mut state,
	)
	.await;
	*area = state.into();
	result.map_err(Into::into)
}
