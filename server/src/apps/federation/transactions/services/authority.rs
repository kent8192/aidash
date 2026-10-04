//! Native adapters delegate authority decisions and durable orchestration to application.
use super::{Manifest, Status};
use crate::{
	Error, Result,
	authorization::{access::Access, identity::SubjectIdentity},
	federation::Federation,
};
use aidash_application::transactions::authority::control as application;
use aidash_domain::identity::execution::ExecutionPrincipal;
use uuid::Uuid;

fn principal(identity: &SubjectIdentity) -> ExecutionPrincipal {
	ExecutionPrincipal {
		credential_id: identity.credential_id,
		tenant: identity.tenant.clone(),
		subject: identity.subject.clone(),
	}
}

pub(in crate::apps::federation::transactions) async fn preflight(
	f: &Federation,
	caller: &str,
	input: &Preflight,
) -> Result<()> {
	application::preflight(
		&crate::bootstrap::transaction_authority_repository(f),
		caller,
		&input.into(),
	)
	.await
	.map_err(Into::into)
}

pub(in crate::apps::federation::transactions) async fn submit(
	f: &Federation,
	identity: &SubjectIdentity,
	manifest: &Manifest,
) -> Result<Status> {
	application::submit(
		&crate::bootstrap::transaction_authority_repository(f),
		&principal(identity),
		manifest,
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}

pub(in crate::apps::federation::transactions) async fn issue(
	f: &Federation,
	manifest: &Manifest,
	node: &str,
) -> Result<()> {
	application::issue(
		&crate::bootstrap::transaction_authority_repository(f),
		manifest,
		node,
	)
	.await
	.map_err(Into::into)
}

pub(in crate::apps::federation::transactions) async fn ticket(
	f: &Federation,
	id: Uuid,
	node: &str,
) -> Result<Preflight> {
	application::ticket(
		&crate::bootstrap::transaction_authority_repository(f),
		id,
		node,
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}

pub(super) async fn admission(
	f: &Federation,
	caller: &str,
	manifest: &Manifest,
) -> Result<Option<Access>> {
	let repository = crate::bootstrap::transaction_authority_repository(f);
	let Some(bound) = application::prepare_admission(&repository, caller, manifest).await? else {
		return Ok(None);
	};
	// The participant must retain this exact locked transaction for its write.
	let mut access = persistence::mapped(f, &bound.request.clone().into()).await?;
	let result = application::check_admission(
		&mut crate::bootstrap::transaction_authority_scope(&mut access),
		&repository,
		&bound,
		caller,
	)
	.await
	.map_err(Error::from);
	if let Err(error) = result {
		return access.finish(Err(error)).await;
	}
	Ok(Some(access))
}

pub(in crate::apps::federation::transactions) async fn require_owner(
	f: &Federation,
	identity: &SubjectIdentity,
	id: Uuid,
) -> Result<Origin> {
	application::require_owner(
		&crate::bootstrap::transaction_authority_repository(f),
		&principal(identity),
		id,
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}

pub(in crate::apps::federation::transactions) async fn manage(
	f: &Federation,
	identity: &SubjectIdentity,
	state: &Status,
	action: &str,
) -> Result<()> {
	application::manage(
		&crate::bootstrap::transaction_authority_repository(f),
		&principal(identity),
		&state.into(),
		action,
	)
	.await
	.map_err(Into::into)
}

pub(in crate::apps::federation::transactions) async fn read_access(
	f: &Federation,
	caller: &str,
	input: &Preflight,
) -> Result<()> {
	application::read_access(
		&crate::bootstrap::transaction_authority_repository(f),
		caller,
		&input.into(),
	)
	.await
	.map_err(Into::into)
}

use crate::apps::federation::transactions::repositories::authority::persistence;
pub(crate) use crate::apps::federation::transactions::serializers::authority::{Origin, Preflight};
pub(in crate::apps::federation::transactions) use persistence::pending_peer;
pub(crate) use persistence::{control, pending};
pub(in crate::apps::federation::transactions) use persistence::{scoped, settle};
