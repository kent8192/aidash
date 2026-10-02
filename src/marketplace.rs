//! Tenant-owned distribution and immutable, separately approved installations.
//! Legacy node-wide Marketplace routes remain in `api`/`registry`.
mod api;
mod definitions;
mod distribution;
pub(crate) mod events;
mod installations;
mod storage;

pub use api::routes;
pub(crate) use installations::{active, catalog_owner, propagate_provenance};
pub(crate) use storage::lock as lock_catalog;

use crate::registry::{EntityRef, Entry, Localized, Package};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = MarketplaceProjection)]
pub struct Projection {
	pub contract: u8,
	pub tenant: String,
	pub installation: String,
	pub revision: i64,
}

#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = MarketplacePublish)]
pub struct Publish {
	pub source: EntityRef,
	pub package_id: String,
	pub author: String,
	#[serde(default)]
	pub permissions: Vec<String>,
	#[serde(default)]
	pub dependencies: Vec<EntityRef>,
	pub idempotency_key: Uuid,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[schema(as = MarketplaceSummary)]
pub struct Summary {
	pub key: String,
	pub repository: String,
	pub owner_tenant: String,
	pub package_id: String,
	pub version: String,
	pub kind: String,
	pub name: Localized,
	pub description: Localized,
	pub author: String,
	pub capabilities: Vec<String>,
	pub permissions: Vec<String>,
	pub languages: Vec<String>,
	pub digest: String,
	pub actions: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[schema(as = MarketplaceDetail)]
pub struct Detail {
	pub summary: Summary,
	pub manifest: Package,
	pub audience: Audience,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[schema(as = MarketplaceAudience)]
pub struct Audience {
	pub revision: i64,
	pub tenants: BTreeSet<String>,
}
#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = MarketplaceAudienceInput)]
pub struct AudienceInput {
	pub expected_revision: i64,
	pub tenants: BTreeSet<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
struct ConsentEdge {
	source: String,
	redistributor: String,
	grant: SourceGrant,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum SourceGrant {
	Consent,
	Audience,
}
#[derive(Clone, Serialize, Deserialize)]
struct Version {
	key: String,
	repository: String,
	owner_tenant: String,
	package_id: String,
	version: String,
	kind: String,
	publisher: String,
	source: EntityRef,
	manifest_source: String,
	digest: String,
	dependencies: Vec<Dependency>,
	lineage: BTreeSet<ConsentEdge>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Dependency {
	reference: EntityRef,
	kind: String,
	digest: String,
	package: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = MarketplaceDependencyBinding)]
pub struct DependencyBinding {
	pub source: EntityRef,
	pub target: EntityRef,
}
#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = MarketplaceInstall)]
pub struct Install {
	pub digest: String,
	#[serde(default = "crate::domain::empty_object")]
	pub config: Value,
	#[serde(default)]
	pub bindings: Vec<DependencyBinding>,
	pub idempotency_key: Uuid,
}
#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = MarketplaceConfigure)]
pub struct Configure {
	pub expected_revision: i64,
	pub config: Value,
	#[serde(default)]
	pub bindings: Vec<DependencyBinding>,
	pub idempotency_key: Uuid,
}
#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[schema(as = MarketplaceInstallation)]
pub struct Installation {
	pub id: String,
	pub tenant: String,
	pub package_key: String,
	pub latest_revision: i64,
	pub active_revision: Option<i64>,
	pub activation_revision: i64,
}
#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[schema(as = MarketplaceInstallationRevision)]
pub struct InstallationRevision {
	pub installation: Installation,
	pub revision: i64,
	pub entry: Entry,
	pub digest: String,
	pub config: Value,
	pub dependencies: Vec<EntityRef>,
	pub bindings: Vec<DependencyBinding>,
	pub approved: bool,
	pub actions: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Revision {
	installation: String,
	tenant: String,
	revision: i64,
	entry: Entry,
	digest: String,
	config: Value,
	dependencies: Vec<EntityRef>,
	bindings: Vec<DependencyBinding>,
	// Retain the verified original source independently of mutable distribution.
	source: Version,
}
#[derive(Serialize, Deserialize, Clone, ToSchema)]
#[schema(as = MarketplaceCompatibility)]
pub struct Compatibility {
	pub enabled: bool,
	pub revision: i64,
	pub contract: u8,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = MarketplaceCompatibilityInput)]
pub struct CompatibilityInput {
	pub enabled: bool,
	pub expected_revision: i64,
	pub compatible_instances_confirmed: bool,
}
#[derive(Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = MarketplaceActivate)]
pub struct Activate {
	pub tenant: String,
	pub revision: i64,
	pub expected_activation_revision: i64,
	pub expected_catalog_revision: i64,
	pub enabled: bool,
}
#[derive(Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = MarketplaceAdopt)]
pub struct Adopt {
	pub tenant: String,
	pub source: EntityRef,
	pub idempotency_key: Uuid,
}

/// All transitive bindings are checked at each worker boundary. The immutable
/// local references themselves never follow a newer active installation alias.
pub(crate) async fn check_pinned(
	access: &mut crate::authorization::access::Access,
	entry: &Entry,
) -> crate::Result<()> {
	if let Some(p) = &entry.installation {
		let rev = installations::revision(&mut access.tx, &p.installation, p.revision).await?;
		for dependency in rev.dependencies {
			crate::authorization::catalog::entry(access, &dependency, "registry.read").await?;
		}
	}
	Ok(())
}

pub(crate) async fn registry_response(
	store: &crate::store::Store,
	identity: &crate::authorization::identity::SubjectIdentity,
	entries: Vec<Entry>,
	single: bool,
) -> crate::Result<axum::response::Response> {
	use crate::{
		Error,
		authorization::{access::Access, catalog},
	};
	use axum::response::IntoResponse;
	if entries.iter().all(|entry| entry.installation.is_none()) {
		return if single {
			Ok(axum::Json(entries.into_iter().next().ok_or(Error::Forbidden)?).into_response())
		} else {
			Ok(axum::Json(entries).into_response())
		};
	}
	let mut access = Access::begin(store, identity).await?;
	storage::lock(&mut access.tx, false).await?;
	let result = async {
		let mut output = vec![];
		for entry in entries {
			let checked = async {
				// Exact retained revisions remain readable only while the
				// Marketplace compatibility contract is enabled.
				if entry.installation.is_some() {
					storage::gate(&mut access.tx).await?;
				}
				let entry = catalog::entry(
					&mut access,
					&definitions::reference(&entry),
					"registry.read",
				)
				.await?;
				check_pinned(&mut access, &entry).await?;
				if !single && !active(&mut access, &entry).await? {
					return Err(Error::Forbidden);
				}
				Ok(entry)
			}
			.await;
			match checked {
				Ok(entry) => output.push(entry),
				Err(Error::Forbidden) if !single => {}
				Err(error) => return Err(error),
			}
		}
		if single {
			Ok(serde_json::to_value(
				output.into_iter().next().ok_or(Error::Forbidden)?,
			)?)
		} else {
			Ok(serde_json::to_value(output)?)
		}
	}
	.await;
	api::handoff(store, access, result).await
}
pub(crate) async fn state_response(
	store: &crate::store::Store,
	identity: &crate::authorization::identity::SubjectIdentity,
	mut state: crate::api_schema::StateResponse,
) -> crate::Result<axum::response::Response> {
	use crate::{
		Error,
		authorization::{access::Access, catalog},
	};
	use axum::response::IntoResponse;
	if state
		.registry
		.iter()
		.all(|entry| entry.installation.is_none())
		&& !state
			.events
			.iter()
			.any(|event| event.kind.starts_with("marketplace."))
	{
		return Ok(axum::Json(state).into_response());
	}
	let mut access = Access::begin(store, identity).await?;
	storage::lock(&mut access.tx, false).await?;
	let result = async {
		let mut entries = vec![];
		for entry in state.registry {
			let checked = async {
				let entry = catalog::entry(
					&mut access,
					&definitions::reference(&entry),
					"registry.read",
				)
				.await?;
				check_pinned(&mut access, &entry).await?;
				if !active(&mut access, &entry).await? {
					return Err(Error::Forbidden);
				}
				Ok(entry)
			}
			.await;
			match checked {
				Ok(entry) => entries.push(entry),
				Err(Error::Forbidden) => {}
				Err(e) => return Err(e),
			}
		}
		state.registry = entries;
		let mut visible_events = vec![];
		for event in state.events {
			if !event.kind.starts_with("marketplace.")
				|| events::visible(&mut access, &event, &store.node_id).await?
			{
				visible_events.push(events::recipient_event(event));
			}
		}
		state.events = visible_events;
		Ok(state)
	}
	.await;
	api::handoff(store, access, result).await
}

#[derive(Serialize, utoipa::ToSchema)]
#[schema(as = MarketplacePublicationPreview)]
pub struct PublicationPreview {
	pub allowed: bool,
	pub version: Option<String>,
}
