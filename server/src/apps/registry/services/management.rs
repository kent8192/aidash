//! Management use cases.
use crate::apps::registry::serializers::contracts::{McpToolHint, McpToolHintRequest};
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
	/// The versions in the subject's Tenant it may read, each with its approval
	/// state and history; a page covers 100 stored versions.
	pub(crate) async fn registry_versions(
		&self,
		actor: Actor,
		page: crate::apps::registry::serializers::contracts::VersionPage,
	) -> Result<crate::apps::registry::serializers::contracts::RegistryVersions> {
		let f = self.runtime.clone();
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		let mut access = Access::begin(&f.store, &identity).await?;
		let result = crate::apps::registry::repositories::versions::page(
			&mut access,
			&f.config.node_id,
			page.offset,
		)
		.await;
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

	/// What the MCP server at `request.endpoint` says about one tool, read for
	/// the registration form. It needs `registry.create`, because its only use
	/// is to register that tool, and stores nothing: the hint never makes an
	/// operation read-only (ADR 0078). The server is reached like a Keyless
	/// endpoint (ADR 0036), so the Tenant cannot aim it at a private address
	/// the Deployment does not permit.
	pub(crate) async fn mcp_tool_hint(
		&self,
		actor: Actor,
		request: McpToolHintRequest,
	) -> Result<McpToolHint> {
		let f = self.runtime.clone();
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		let mut access = Access::begin(&f.store, &identity).await?;
		let resource = access.resource(
			"registry",
			format!(
				"tool/{}",
				aidash_domain::identity::tenant_administration::COLLECTION
			),
			serde_json::json!({"kind": "tool"}),
		);
		let result = access.require(&resource, "registry.create").await;
		access.finish(result).await?;
		let read_only_hint = aidash_integrations::tools::mcp_read_only_hint(
			&f.store.provider_clients,
			&f.store.keyless,
			&request.endpoint,
			&request.tool_name,
		)
		.await
		.map_err(|error| match error {
			// The reason can name hosts and ports the caller should not probe, and
			// an address the Deployment does not permit reads like an unreachable one.
			error @ (aidash_application::Error::External(_)
			| aidash_application::Error::Forbidden) => {
				tracing::debug!(%error, "MCP tool hint unavailable");
				Error::Invalid("the MCP server could not be read".into())
			}
			other => other.into(),
		})?;
		Ok(McpToolHint { read_only_hint })
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
		let resource = access.resource(
			"registry",
			format!(
				"skill/{}",
				aidash_domain::identity::tenant_administration::COLLECTION
			),
			serde_json::json!({"kind": "skill"}),
		);
		let result = access.require(&resource, "registry.create").await;
		access.finish(result).await?;
		crate::skill_import::import(request).await
	}
}

use reinhardt::Response;
