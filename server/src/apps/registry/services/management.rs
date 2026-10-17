//! Management use cases.
use crate::{
	Error, Result,
	apps::registry::repositories::requests::TenantRegistration,
	authorization::{access::Access, catalog, identity::Actor},
	federation::Federation,
	registry::{EntityRef, Entry, Search},
};
use http::HeaderMap;
use reinhardt::injectable;
use uuid::Uuid;

#[derive(Clone)]
pub struct RegistryManagement {
	runtime: Federation,
}
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> RegistryManagement {
	RegistryManagement { runtime }
}
impl RegistryManagement {
	/// Definitions belong to Tenants (ADR 0029): a Deployment Administrator reads none.
	pub(crate) async fn registry_list(&self, actor: Actor, search: Search) -> Result<Response> {
		let f = self.runtime.clone();
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		let entries = catalog::list(&f.store, &identity, &search).await?;
		crate::marketplace::registry_response(&f.store, &identity, entries, false).await
	}
	pub(crate) async fn registry_get(
		&self,
		actor: Actor,
		(id, version): (String, String),
	) -> Result<Response> {
		let f = self.runtime.clone();
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		let entry = catalog::get(&f.store, &identity, &EntityRef { id, version }).await?;
		crate::marketplace::registry_response(&f.store, &identity, vec![entry], true).await
	}
	/// Every version in the subject's Tenant with its approval state and
	/// history, under `registry.read` (100 per page).
	pub(crate) async fn registry_versions(
		&self,
		actor: Actor,
		page: crate::apps::registry::serializers::contracts::VersionPage,
	) -> Result<Vec<crate::apps::registry::serializers::contracts::RegistryVersion>> {
		let f = self.runtime.clone();
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		let mut access = Access::begin(&f.store, &identity).await?;
		let result =
			crate::apps::registry::repositories::versions::page(&mut access, page.offset).await;
		access.finish(result).await
	}
	/// A new definition or version in the subject's Tenant, under `registry.create`.
	pub(crate) async fn registry_create(
		&self,
		actor: Actor,
		headers: HeaderMap,
		entry: Entry,
	) -> Result<Entry> {
		let f = self.runtime.clone();
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		let key = headers
			.get("idempotency-key")
			.map(|value| {
				value
					.to_str()
					.ok()
					.and_then(|value| Uuid::parse_str(value).ok())
					.ok_or_else(|| Error::Invalid("Idempotency-Key must be a UUID".into()))
			})
			.transpose()?;
		let mut access = Access::begin(&f.store, &identity).await?;
		let result = aidash_application::registry::register(
			&mut TenantRegistration {
				access: &mut access,
				store: &f.store,
			},
			&f.registry.validation(),
			entry,
			key,
			true,
			&f.config.node_id,
		)
		.await
		.map_err(Into::into);
		access.finish(result).await
	}

	/// Fetching a public Skill snapshot needs `registry.create`, because its only
	/// use is to register it; the fixed GitHub host bounds what it reaches.
	pub(crate) async fn skill_import(
		&self,
		actor: Actor,
		request: crate::skill_import::ImportRequest,
	) -> Result<crate::skill_import::ImportResult> {
		let f = self.runtime.clone();
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		let mut access = Access::begin(&f.store, &identity).await?;
		let resource = access.resource("registry", "skill/*", serde_json::json!({"kind": "skill"}));
		let result = access.require(&resource, "registry.create").await;
		access.finish(result).await?;
		crate::skill_import::import(request).await
	}
}

use reinhardt::Response;
