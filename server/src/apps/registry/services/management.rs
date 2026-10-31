//! Management use cases.
use crate::apps::identity::services::tenant_secret_readiness;
use crate::apps::registry::serializers::agent_plugin_import::{
	AgentPluginImportResult, PluginImportRequest,
};
use crate::apps::registry::serializers::contracts::{McpToolHint, McpToolHintRequest};
use crate::apps::registry::services::agent_plugin_import;
use crate::{
	Error, Result,
	apps::registry::repositories::requests::TenantRegistration,
	authorization::{access::Access, catalog, identity::Actor},
	federation::Federation,
	registry::{EntityRef, Entry, Search},
};
use aidash_application::tenant_secrets::readiness::{Fact, Kind, Target};
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
			f.config.deployment.as_str(),
			page.node,
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
			f.config.deployment.as_str(),
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

	/// See [`crate::apps::registry::services::agent_plugin_import::import`].
	pub(crate) async fn agent_plugin_import(
		&self,
		actor: Actor,
		request: PluginImportRequest,
	) -> Result<AgentPluginImportResult> {
		let Actor::Subject(ref identity) = actor else {
			return Err(Error::Forbidden);
		};
		let source = crate::bootstrap::agent_plugin_import(&self.runtime.config, &identity.tenant)?;
		agent_plugin_import::import(&self.runtime, &source, actor, request).await
	}

	/// Readiness facts for the Tenant Secrets this Agent version's bound tools
	/// and External services name (decision 5 of #310). A bound tool or
	/// External service that names no Tenant Secret needs none and has no fact.
	/// A fact never carries a value, a Fingerprint or the last four characters.
	pub(crate) async fn secret_readiness(
		&self,
		actor: Actor,
		(id, version): (String, String),
	) -> Result<Vec<Fact>> {
		let f = self.runtime.clone();
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		let tenant = aidash_domain::tenancy::TenantName::new(&identity.tenant)?;
		let mut access = Access::begin(&f.store, &identity).await?;
		let result = async {
			// Readiness is a management view of an Agent version, so it needs
			// `registry.read` but not a Catalog Approval, which admits Runs. The
			// collection decision runs before lookup, so a missing Agent is 404
			// only for a caller that may read the Registry at all.
			let collection = access.resource(
				"registry",
				format!(
					"agent/{}",
					aidash_domain::identity::tenant_administration::COLLECTION
				),
				serde_json::json!({"kind": "agent"}),
			);
			if !access.decide(&collection, "registry.read").await? {
				return Err(Error::Forbidden);
			}
			let agent = f.registry.get(&tenant, &id, &version).await?;
			if agent.kind != "agent" {
				return Err(Error::NotFound("Agent".into()));
			}
			let resource = catalog::resource(&access, &agent);
			if !access.decide(&resource, "registry.read").await? {
				return Err(Error::Forbidden);
			}
			let node = access.node_id.clone();
			let snapshot = crate::apps::registry::repositories::bindings::snapshot(
				&mut access.tx,
				&node,
				&agent,
				false,
				f.store.provider_credentials.is_some(),
			)
			.await?;
			let mut targets = Vec::new();
			for resolved in snapshot.definitions {
				let kind = match resolved.definition.kind.as_str() {
					"tool" => Kind::Tool,
					"external_service" => Kind::ExternalService,
					_ => continue,
				};
				let resource = catalog::resource(&access, &resolved.definition);
				if !access.decide(&resource, "registry.read").await? {
					return Err(Error::Forbidden);
				}
				// A bound tool or External service that names no shared Tenant Secret
				// (none, or a personal-only one) has no readiness fact.
				let Some((tenant_secret_name, endpoint)) =
					aidash_domain::tenant_secrets::definition_shared_target(&resolved.definition)
				else {
					continue;
				};
				targets.push(Target {
					item: format!("{}@{}", resolved.definition.id, resolved.definition.version),
					kind,
					tenant_secret_name: Some(tenant_secret_name),
					endpoint: Some(endpoint),
				});
			}
			targets.sort_by(|left, right| left.item.cmp(&right.item));
			let can_manage = tenant_secret_readiness::can_manage(&mut access).await?;
			tenant_secret_readiness::facts(&f.store, &identity.tenant, can_manage, targets).await
		}
		.await;
		access.finish(result).await
	}
}

use reinhardt::Response;
