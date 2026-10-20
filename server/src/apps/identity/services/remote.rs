//! Durable source authority for remote admission. A prepared grant is pinned to
//! a task revision and exact receiver definitions; possession never bypasses
//! current policy, credential, task, peer or receiver checks.
#[path = "remote/execution.rs"]
pub(crate) mod execution;

use super::{access::Access, identity::Actor, peer::execution::Inspection};
#[cfg(test)]
use crate::registry::Search;
use crate::{Result, domain::Task, federation::Federation, registry::EntityRef, store::Store};
use aidash_domain::federation::execution::Described;
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
		&crate::bootstrap::peer_home_execution_repository(f),
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
		&crate::bootstrap::peer_home_execution_repository(f),
		node,
		id,
		command,
		revision_race,
	)
	.await
	.map(|(scope, description)| (scope.into_access(), description))
	.map_err(Into::into)
}

/// The corpus a remote Run's semantic read draws from, as the Home records it
/// for each remote retrieval. Read after the live-grant checks above, and only
/// when the grant carries Home semantic access.
///
/// The digest covers every live entry of the Workspace, as it does for a local
/// Run, so it also moves when an entry the executor may not retrieve changes.
/// The receiver learns only that the Workspace corpus changed, never content.
async fn corpus_digest(store: &Store, description: &Description) -> Result<Option<String>> {
	if description.semantic.disabled() {
		return Ok(None);
	}
	// Home reads its own Workspace, in the Tenant that issued the grant.
	let scope = aidash_application::tenancy::TenancyScope::tenant(&description.source_tenant)?;
	crate::semantic::services::memory_context::remote_corpus_digest(
		store.pool.in_scope(&scope),
		description,
	)
	.await
	.map(Some)
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
	pub async fn agent_inspection(
		&self,
		actor: Actor,
		task_id: Uuid,
		input: aidash_domain::federation::execution::AgentInspectionInput,
	) -> Result<aidash_domain::federation::execution::AgentMemoryRequirements> {
		aidash_application::authorization::source::grants::agent_inspection(
			&crate::bootstrap::home_execution_repository(&self.runtime, actor),
			task_id,
			input,
		)
		.await
		.map_err(Into::into)
	}

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
	/// The pinned Description, plus the Workspace's current corpus digest.
	pub(crate) async fn describe(
		&self,
		headers: HeaderMap,
		input: VerifyInput,
	) -> Result<Described> {
		let f = self.runtime.clone();
		let (access, description) = description_lease(
			&f,
			crate::apps::identity::services::http_auth::peer_node(&headers)?,
			input.grant_id,
		)
		.await?;
		let description = access.finish(Ok(description)).await?;
		let corpus_digest = corpus_digest(&f.store, &description).await?;
		Ok(Described {
			description,
			corpus_digest,
		})
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
		let result = aidash_application::authorization::source::snapshot::read(
			&mut crate::bootstrap::source_snapshot_scope(&mut access),
			description.grant_id,
			description.task.workspace_id,
		)
		.await
		.map_err(Into::into);
		access.finish(result).await
	}
}

#[path = "remote/deployment_administrator.rs"]
pub(crate) mod deployment_administrator;

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
