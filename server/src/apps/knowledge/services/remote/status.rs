//! Native summary adapters keep the original HTTP schemas and authority scope.
use super::{Binding, Failure};
use crate::{
	Error, Result,
	authorization::{access::Access, identity::Actor},
	federation::Federation,
	store::Store,
};
pub use aidash_domain::semantic::remote::status::{Provenance, Status};
use serde_json::Value;
use uuid::Uuid;
pub(crate) async fn provenance(
	access: &mut Access,
	node: &str,
	value: Option<Value>,
) -> Result<Option<Provenance>> {
	aidash_application::semantic::remote_status::provenance(
		&mut crate::bootstrap::semantic_status_scope(None, access, node),
		value,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn run_receipt(
	f: Federation,
	actor: Actor,
	id: Uuid,
) -> Result<Option<Provenance>> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = aidash_application::semantic::remote_status::run_receipt(
		&mut crate::bootstrap::semantic_status_scope(
			Some(&f.store),
			&mut access,
			&f.config.node_id,
		),
		id,
	)
	.await
	.map_err(Into::into);
	access.finish(result).await
}
pub(crate) async fn load(
	store: &Store,
	grant: Uuid,
	binding: &Binding,
	reason: Option<Failure>,
) -> Result<Status> {
	aidash_application::semantic::remote_status::load(
		&crate::bootstrap::semantic_status_repository(store),
		grant,
		binding,
		reason,
	)
	.await
	.map_err(Into::into)
}
