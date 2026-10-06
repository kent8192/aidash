//! HTTP commands adapt the shared application use case and commit its source authority lease.
use super::*;
use http::HeaderMap;
pub(crate) async fn handle(f: Federation, headers: HeaderMap, input: Input) -> Result<Value> {
	let node = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	let mut revision_race = false;
	let (mut access, description) =
		super::super::description_lease_mode(&f, node, input.grant_id, true, &mut revision_race)
			.await?;
	access.worker();
	access.read_grant = Some(input.grant_id);
	let agent = &description.inspection.agent;
	let result = aidash_application::authorization::commands::effects::execute(
		&mut crate::bootstrap::remote_command_scope(&f, &mut access),
		aidash_application::authorization::commands::Command {
			grant_id: input.grant_id,
			admission_id: input.admission_id,
			operation: &input.operation,
			data: &input.data,
			node,
			agent: &agent.id,
			version: &agent.version,
		},
		agent,
	)
	.await
	.map_err(Into::into);
	if let Err(error) = &result {
		tracing::warn!(operation=%input.operation,error=%error,"scoped home command rejected");
	}
	access.finish(result).await
}
#[derive(Clone)]
pub struct RemoteCommands {
	pub(crate) runtime: Federation,
}
#[reinhardt::injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> RemoteCommands {
	RemoteCommands { runtime }
}
pub(crate) use crate::apps::identity::serializers::remote_execution_commands::Input;
