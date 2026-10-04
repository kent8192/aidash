//! Durable source authority for remote admission. A prepared grant is pinned to
//! a task revision and exact receiver definitions; possession never bypasses
//! current policy, credential, task, peer or receiver checks.
#[path = "remote/execution.rs"]
pub(crate) mod execution;

use super::{access::Access, identity::Actor, peer::execution::Inspection};
#[cfg(test)]
use crate::registry::Search;
use crate::{Error, Result, domain::Task, federation::Federation, registry::EntityRef};
use reinhardt::injectable;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;

use uuid::Uuid;

// A receiver may describe only the requested Agent's exact direct dependencies.
// Never use peer-provided node IDs or arbitrary resource lists as authority.

pub(crate) async fn live(access: &mut Access, id: Uuid) -> Result<bool> {
	crate::apps::identity::repositories::remote_grants::persistence::live(access, id).await
}

// Only the destination peer may obtain the source-authorized task. Both this
// description and boolean verification share the exact live authority checks.

pub(crate) async fn description_lease(
	f: &Federation,
	node: &str,
	id: Uuid,
) -> Result<(Access, Description)> {
	aidash_application::authorization::source::grants::description(
		&crate::bootstrap::home_execution_repository(f, Actor::Operator),
		node,
		id,
	)
	.await
	.map(|(scope, description)| (scope.into_access(), description))
	.map_err(Into::into)
}
async fn description_lease_mode(
	f: &Federation,
	node: &str,
	id: Uuid,
	command: bool,
	revision_race: &mut bool,
) -> Result<(Access, Description)> {
	aidash_application::authorization::source::grants::description_mode(
		&crate::bootstrap::home_execution_repository(f, Actor::Operator),
		node,
		id,
		command,
		revision_race,
	)
	.await
	.map(|(scope, description)| (scope.into_access(), description))
	.map_err(Into::into)
}

#[cfg(test)]
#[path = "../tests/services_remote_tests.rs"]
mod tests;

pub(crate) use crate::apps::identity::serializers::remote::{Description, VerifyInput};
pub use crate::apps::identity::serializers::remote::{PrepareInput, Prepared};

use http::HeaderMap;

#[derive(Clone)]
pub struct RemoteGrants {
	pub(crate) runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide_remote_grants(#[inject] runtime: Federation) -> RemoteGrants {
	RemoteGrants { runtime }
}

impl RemoteGrants {
	pub async fn prepare(
		&self,
		actor: Actor,
		task_id: Uuid,
		input: PrepareInput,
	) -> Result<Prepared> {
		aidash_application::authorization::source::grants::prepare(
			&crate::bootstrap::home_execution_repository(&self.runtime, actor),
			task_id,
			input,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn revoke(
		&self,
		actor: Actor,
		(task_id, id): (Uuid, Uuid),
	) -> Result<Prepared> {
		aidash_application::authorization::source::grants::revoke(
			&crate::bootstrap::home_execution_repository(&self.runtime, actor),
			task_id,
			id,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn describe(
		&self,
		headers: HeaderMap,
		input: VerifyInput,
	) -> Result<Description> {
		let f = self.runtime.clone();
		let (access, description) = description_lease(
			&f,
			crate::apps::identity::services::http_auth::peer_node(&headers)?,
			input.grant_id,
		)
		.await?;
		access.finish(Ok(description)).await
	}
	pub(crate) async fn verify(&self, headers: HeaderMap, input: VerifyInput) -> Result<bool> {
		let f = self.runtime.clone();
		let (access, _) = description_lease(
			&f,
			crate::apps::identity::services::http_auth::peer_node(&headers)?,
			input.grant_id,
		)
		.await?;
		access.finish(Ok(true)).await
	}
	pub(crate) async fn snapshot(
		&self,
		headers: HeaderMap,
		input: VerifyInput,
	) -> Result<crate::domain::WorkspaceSnapshot> {
		let f = self.runtime.clone();
		let (mut access, description) = description_lease(
			&f,
			crate::apps::identity::services::http_auth::peer_node(&headers)?,
			input.grant_id,
		)
		.await?;
		access.read_grant = Some(description.grant_id);
		let result = async {
			let snapshot = access
				.workspace_snapshot(description.task.workspace_id)
				.await?;
			if !live(&mut access, description.grant_id).await? {
				return Err(Error::Forbidden);
			}
			Ok(snapshot)
		}
		.await;
		access.finish(result).await
	}
}

#[path = "remote/operator.rs"]
pub(crate) mod operator;

#[path = "remote/reads.rs"]
pub(crate) mod reads;

#[path = "remote/semantic.rs"]
pub(crate) mod semantic;

#[cfg(test)]
fn validate(
	inspection: &Inspection,
	node: &str,
	agent: &EntityRef,
	requirements: &Search,
) -> Result<()> {
	aidash_application::federation::admission::validate_inspection(
		&crate::bootstrap::registry_validation(),
		inspection,
		node,
		agent,
		requirements,
	)
	.map_err(Into::into)
}
