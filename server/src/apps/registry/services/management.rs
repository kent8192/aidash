//! Management use cases.
use crate::apps::registry::models::records;
use crate::{
	Error, Result,
	authorization::{catalog, identity::Actor},
	federation::Federation,
	registry::{EntityRef, Entry, Package, PackageRecord, Search},
};
use http::HeaderMap;
use reinhardt::injectable;
use uuid::Uuid;

use crate::apps::registry::serializers::management::InstallInput;

#[derive(Clone)]
pub struct RegistryManagement {
	runtime: Federation,
}
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> RegistryManagement {
	RegistryManagement { runtime }
}
impl RegistryManagement {
	pub(crate) async fn registry_list(&self, actor: Actor, search: Search) -> Result<Response> {
		let f = self.runtime.clone();
		if let Actor::Subject(identity) = actor {
			let entries = catalog::list(&f.store, &identity, &search).await?;
			return crate::marketplace::registry_response(&f.store, &identity, entries, false)
				.await;
		}
		Ok(Response::ok().with_json(&f.registry.list(&search).await?)?)
	}
	pub(crate) async fn registry_get(
		&self,
		actor: Actor,
		(id, version): (String, String),
	) -> Result<Response> {
		let f = self.runtime.clone();
		if let Actor::Subject(identity) = actor {
			let entry = catalog::get(&f.store, &identity, &EntityRef { id, version }).await?;
			return crate::marketplace::registry_response(&f.store, &identity, vec![entry], true)
				.await;
		}
		Ok(Response::ok().with_json(&f.registry.get(&id, &version).await?)?)
	}
	pub(crate) async fn registry_create(&self, headers: HeaderMap, entry: Entry) -> Result<Entry> {
		let f = self.runtime.clone();
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
		f.registry.register_with_event(entry, key).await
	}

	pub(crate) async fn skill_import(
		&self,
		request: crate::skill_import::ImportRequest,
	) -> Result<crate::skill_import::ImportResult> {
		crate::skill_import::import(request).await
	}
	pub(crate) async fn marketplace(&self, query: Search) -> Result<Vec<PackageRecord>> {
		let f = self.runtime.clone();
		let all = records::packages(f.registry.db).await?;
		Ok(all
			.into_iter()
			.filter(|p| {
				serde_json::from_value::<Package>(p.manifest.clone())
					.is_ok_and(|p| query.matches(&p.entity))
			})
			.collect())
	}
	pub(crate) async fn package_publish(&self, package: Package) -> Result<PackageRecord> {
		let f = self.runtime.clone();
		let p = f.registry.publish(package).await?;
		Ok(p)
	}
	pub(crate) async fn package_install(
		&self,
		(id, version): (String, String),
		input: InstallInput,
	) -> Result<Entry> {
		let f = self.runtime.clone();
		let e = f
			.registry
			.install(&id, &version, &input.digest, input.config)
			.await?;
		Ok(e)
	}
}

use reinhardt::Response;
