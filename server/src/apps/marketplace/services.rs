//! Tenant-owned distribution and immutable, separately approved installations.
//! Legacy node-wide Marketplace routes remain in the registry app.
mod distribution;
pub(crate) mod events;
pub(crate) mod installations;
pub(crate) mod management;
pub(crate) mod migration_seed;
pub(crate) mod storage;

pub(crate) use installations::{active, propagate_provenance};
pub(crate) use storage::lock as lock_catalog;

use crate::registry::{EntityRef, Entry};
use uuid::Uuid;

use serde_json::Value;

/// Recheck the immutable dependency graph at each worker boundary.
pub(crate) async fn check_pinned(
	access: &mut crate::authorization::access::Access,
	entry: &Entry,
) -> crate::Result<()> {
	aidash_application::marketplace::installations::check_pinned(
		&mut crate::bootstrap::marketplace_definitions_scope(access),
		entry,
	)
	.await
	.map_err(Into::into)
}

pub(crate) async fn registry_response(
	store: &crate::store::Store,
	identity: &crate::authorization::identity::SubjectIdentity,
	entries: Vec<Entry>,
	single: bool,
) -> crate::Result<Response> {
	use crate::{Error, authorization::access::Access};
	if entries.iter().all(|entry| entry.installation.is_none()) {
		return if single {
			Ok(Response::ok().with_json(&entries.into_iter().next().ok_or(Error::Forbidden)?)?)
		} else {
			Ok(Response::ok().with_json(&entries)?)
		};
	}
	let mut access = Access::begin(store, identity).await?;
	storage::lock(&mut access.tx, false).await?;
	let result = async {
		let output = aidash_application::marketplace::events::registry_entries(
			&mut crate::bootstrap::marketplace_distribution_scope(&mut access),
			entries,
			single,
			&store.node_id,
		)
		.await?;
		if single {
			Ok(serde_json::to_value(
				output.into_iter().next().ok_or(Error::Forbidden)?,
			)?)
		} else {
			Ok(serde_json::to_value(output)?)
		}
	}
	.await;
	management::handoff(store, access, result).await
}
pub(crate) async fn state_response(
	store: &crate::store::Store,
	identity: &crate::authorization::identity::SubjectIdentity,
	mut state: crate::apps::execution::serializers::state::StateResponse,
) -> crate::Result<Response> {
	use crate::authorization::access::Access;
	if state
		.registry
		.iter()
		.all(|entry| entry.installation.is_none())
		&& !state
			.events
			.iter()
			.any(|event| event.kind.starts_with("marketplace."))
	{
		return Ok(Response::ok().with_json(&state)?);
	}
	let mut access = Access::begin(store, identity).await?;
	storage::lock(&mut access.tx, false).await?;
	let result = async {
		let mut scope = crate::bootstrap::marketplace_distribution_scope(&mut access);
		state.registry = aidash_application::marketplace::events::state_registry(
			&mut scope,
			state.registry,
			&store.node_id,
		)
		.await?;
		state.events = aidash_application::marketplace::events::state_events(
			&mut scope,
			state.events,
			&store.node_id,
		)
		.await?;
		Ok(state)
	}
	.await;
	management::handoff(store, access, result).await
}

pub use super::serializers::contracts::{
	Activate, Adopt, Audience, AudienceInput, Compatibility, CompatibilityInput, Configure,
	DependencyBinding, Detail, Install, Installation, InstallationRevision, Projection,
	PublicationPreview, Publish, Summary,
};
use reinhardt::Response;
