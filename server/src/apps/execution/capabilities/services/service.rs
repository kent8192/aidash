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
/// Start a Completion Gate Check (#112) under the Run's bound shell capability.
pub(crate) async fn verify(
	store: &Store,
	access: &mut Access,
	run: &Run,
	input: Value,
	key: &str,
) -> Result<Value> {
	application::verify(
		&mut crate::bootstrap::file_scope(Some(store), access, Some(run)),
		&run.metadata(),
		input,
		key,
	)
	.await
	.map_err(Into::into)
}
/// The current result of a Completion Gate Check the Run started.
pub(crate) async fn verification(
	store: &Store,
	access: &mut Access,
	run: &Run,
	operation: uuid::Uuid,
) -> Result<Value> {
	application::verification(
		&mut crate::bootstrap::file_scope(Some(store), access, Some(run)),
		&run.metadata(),
		operation,
		&operation.to_string(),
	)
	.await
	.map_err(Into::into)
}
/// The Run's Working Area; `None` when the Run has none.
pub(crate) async fn working_area(
	store: &Store,
	access: &mut Access,
	run: &Run,
) -> Result<Option<aidash_domain::capabilities::sessions::Area>> {
	use aidash_application::ports::capabilities::files::FileScopePort as _;
	match crate::bootstrap::file_scope(Some(store), access, Some(run))
		.for_run(&run.metadata())
		.await
	{
		Ok(area) => Ok(Some(area)),
		Err(aidash_application::Error::NotFound(_)) => Ok(None),
		Err(error) => Err(error.into()),
	}
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
