//! Peer authentication never substitutes for current mapped tenant authority.
use crate::{
	Error, Result,
	ports::authorization::peer::mappings::{MappingAccess, MappingRepository, MappingWrite},
};
use aidash_domain::{
	identity::peer_mapping::{Mapping, PeerMappingInput},
	policy::identifier,
};
use serde_json::json;
// An unusable mapped credential denies authority without challenging the authenticated caller.
fn authority_error(error: Error) -> Error {
	match error {
		Error::Unauthorized => Error::Forbidden,
		other => other,
	}
}
pub async fn write<R: MappingRepository>(
	repository: &R,
	tenant: &str,
	input: PeerMappingInput,
) -> Result<Mapping> {
	input.validate(tenant, repository.node_id())?;
	if input.enabled {
		repository.enabled_peer(&input.source_node).await?;
	}
	input.require_existing_revocation()?;
	let mut scope = repository.begin_write(input.enabled).await?;
	scope.policy(tenant).await?;
	let subject = scope
		.credential_subject(tenant, input.credential_id)
		.await?
		.ok_or(Error::Forbidden)?;
	if input.enabled {
		scope
			.lock_credential(tenant, input.credential_id, &subject)
			.await
			.map_err(authority_error)?;
	}
	let mapping = if input.expected_revision == 0 {
		scope.insert(tenant, &input).await?
	} else {
		scope.update(tenant, &input).await?
	}
	.ok_or_else(|| Error::Conflict("peer mapping revision or tenant changed".into()))?;
	scope.history(&mapping).await?;
	scope.commit().await?;
	Ok(mapping)
}
pub async fn access<R: MappingRepository>(
	repository: &R,
	node: &str,
	tenant: &str,
	subject: &str,
	exclusive: bool,
) -> Result<R::Access> {
	identifier(tenant)?;
	identifier(subject)?;
	let mapping = repository
		.resolved(node, tenant, subject)
		.await?
		.ok_or(Error::Forbidden)?;
	let local_subject = repository
		.credential_subject(&mapping)
		.await?
		.ok_or(Error::Forbidden)?;
	let mut scope = repository
		.begin_access(&mapping, &local_subject, exclusive)
		.await
		.map_err(authority_error)?;
	if scope.current(node, tenant, subject).await?.as_ref() != Some(&mapping) {
		return Err(Error::Forbidden);
	}
	if !scope.peer_enabled(node).await? {
		return Err(Error::Forbidden);
	}
	scope.environment()["transport"] = json!("federation");
	scope.environment()["source_node"] = json!(node);
	*scope.context() = json!({"source_node":node,"source_tenant":tenant,"source_subject":subject});
	Ok(scope)
}
#[cfg(test)]
mod tests;
