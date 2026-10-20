//! Peer authentication never substitutes for current mapped tenant authority.
use crate::federation::inbound::{Acting, Admitted};
use crate::tenancy::TenancyScope;
use crate::{
	Error, Result,
	ports::authorization::peer::mappings::{MappingAccess, MappingRepository, MappingWrite},
};
use aidash_domain::{
	identity::peer_mapping::{Mapping, PeerMappingInput},
	nodes::{NodeAddress, NodeName},
	policy::identifier,
	tenancy::TenantName,
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
	input.validate(tenant, repository.deployment())?;
	input.require_existing_revocation()?;
	let tenancy = TenancyScope::tenant(tenant)?;
	let mut scope = repository.begin_write(&tenancy, input.enabled).await?;
	scope.policy(tenant).await?;
	let subject = scope
		.credential_subject(tenant, input.credential_id)
		.await?
		.ok_or(Error::Forbidden)?;
	scope.authorize(tenant, &input, &subject).await?;
	if input.enabled {
		scope
			.enabled_registration(tenant, &input.node, &input.source_node)
			.await?;
	}
	// A disabled mapping may keep a revoked or expired credential, never one
	// that could not be exported.
	scope.exportable(tenant, input.credential_id).await?;
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

/// The receiving authority of an admitted request: the mapped credential's
/// subject for a Node of another Tenant, or the Tenant's own subject.
/// Admission already decided that the sender may reach the Node; this holds
/// the credential, policy and mapping leases for the operation itself.
pub async fn access<R: MappingRepository>(
	repository: &R,
	admitted: &Admitted,
	exclusive: bool,
) -> Result<R::Access> {
	match &admitted.acting {
		Acting::SameTenant { subject } => {
			identifier(subject)?;
			let scope = repository
				.begin_subject(&admitted.tenant, subject, exclusive)
				.await
				.map_err(authority_error)?;
			described(scope, &admitted.sender, "tenant", subject)
		}
		Acting::Mapped { subject, .. } => {
			mapped(
				repository,
				&admitted.tenant,
				&admitted.receiver.node,
				&admitted.sender,
				subject,
				exclusive,
			)
			.await
		}
		Acting::Node => Err(Error::Forbidden),
	}
}

/// The authority of the local subject that `source`'s `subject` is mapped to
/// at `node`.
pub async fn mapped<R: MappingRepository>(
	repository: &R,
	tenant: &TenantName,
	node: &NodeName,
	source: &NodeAddress,
	subject: &str,
	exclusive: bool,
) -> Result<R::Access> {
	identifier(subject)?;
	let mapping = repository
		.resolved(tenant, node, source, subject)
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
	if scope.current(tenant, node, source, subject).await?.as_ref() != Some(&mapping) {
		return Err(Error::Forbidden);
	}
	if !scope.registered(tenant, node, source).await? {
		return Err(Error::Forbidden);
	}
	let source_scope = if source.deployment == *repository.deployment() {
		"deployment"
	} else {
		"remote"
	};
	described(scope, source, source_scope, subject)
}

/// What the receiving policy and audit know about where the request came from.
fn described<A: MappingAccess>(
	mut scope: A,
	source: &NodeAddress,
	source_scope: &str,
	subject: &str,
) -> Result<A> {
	scope.environment()["transport"] = json!("federation");
	scope.environment()["source_node"] = json!(source.to_string());
	scope.environment()["source_scope"] = json!(source_scope);
	*scope.context() = json!({
		"source_node": source.to_string(),
		"source_subject": subject,
	});
	Ok(scope)
}
#[cfg(test)]
mod tests;
