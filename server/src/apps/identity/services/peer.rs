//! Explicit inbound identity mappings. Peer authentication alone grants no
//! tenant authority, and local subject bearer tokens never cross this boundary.
use aidash_application::federation::inbound::Admitted;
use reinhardt::injectable;
#[path = "peer/admission.rs"]
pub(crate) mod admission;
#[path = "peer/discovery.rs"]
pub(crate) mod discovery;
#[path = "peer/execution.rs"]
pub(crate) mod execution;
#[path = "peer/graph.rs"]
pub(crate) mod graph;
#[path = "peer/reads.rs"]
pub(crate) mod reads;

use super::{access::Access, catalog, policy::identifier};
use crate::{
	Result,
	federation::Federation,
	registry::{Entry, Search},
};

use serde_json::json;

pub async fn write(f: &Federation, tenant: &str, input: PeerMappingInput) -> Result<PeerMapping> {
	write_as(f, tenant, input, None).await
}

/// Writes a Peer Mapping as the host (fixtures and `manage` work, which no
/// HTTP route reaches), or as a Tenant Administrator acting through its
/// Mapping (#359). A Deployment Administrator has no HTTP route to either (#484).
pub(crate) async fn write_as(
	f: &Federation,
	tenant: &str,
	input: PeerMappingInput,
	administrator: Option<std::sync::Arc<crate::apps::identity::models::TenantAdministrator>>,
) -> Result<PeerMapping> {
	aidash_application::authorization::peer::mappings::write(
		&crate::apps::identity::repositories::peer_mappings::Repository {
			runtime: f,
			administrator,
		},
		tenant,
		input,
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}

/// The Sender of work that a Tenant's Node does as itself, naming no subject:
/// a callback that a grant or a transaction authorizes. The Node is the
/// Tenant's default Node until Runs, Threads and Tasks name theirs (#241
/// step 3); this is the one place that changes then.
pub(crate) fn node_sender(tenant: &str) -> Result<aidash_domain::federation::route::Sender> {
	Ok(aidash_domain::federation::route::Sender {
		tenant: aidash_domain::tenancy::TenantName::new(tenant)?,
		node: None,
		subject: None,
	})
}

/// The Sender for a cross-Node generation or semantic operation to `peer`,
/// found from the grant and admission it names: this Deployment's Tenant of
/// that work.
pub(crate) async fn usage_sender(
	f: &Federation,
	admission: uuid::Uuid,
	grant: uuid::Uuid,
	peer: &str,
) -> Result<aidash_domain::federation::route::Sender> {
	let scope = f.store.remote_tenancy(admission, grant).await?;
	// A Node of this Deployment that is the peer is the other end of the
	// exchange, never the sender.
	let peer = aidash_domain::nodes::NodeAddress::parse(peer)
		.ok()
		.filter(|peer| peer.deployment == f.config.deployment)
		.map(|peer| peer.node);
	let node = f
		.store
		.remote_node(&scope, admission, grant, peer.as_ref())
		.await?;
	Ok(aidash_domain::federation::route::Sender {
		node: Some(node),
		..scope_sender(&scope)?
	})
}

/// The same for the Tenant a scope is opened in.
pub(crate) fn scope_sender(
	scope: &aidash_application::tenancy::TenancyScope,
) -> Result<aidash_domain::federation::route::Sender> {
	match scope {
		aidash_application::tenancy::TenancyScope::Tenant(tenant) => {
			Ok(aidash_domain::federation::route::Sender {
				tenant: tenant.clone(),
				node: None,
				subject: None,
			})
		}
		_ => Err(crate::Error::Forbidden),
	}
}

/// The authority that `subject` of `source` is mapped to at the receiving Node
/// `receiver`, for a request whose subject comes from a description the source
/// signed rather than from the token.
pub(crate) async fn mapped_access(
	f: &Federation,
	receiver: &aidash_domain::nodes::NodeName,
	source: &str,
	subject: &str,
) -> Result<Access> {
	mapped_access_mode(f, receiver, source, subject, false).await
}

/// Grant and transfer callbacks recover the described subject at the receiving
/// Node. Siblings in one Tenant use its policy without a Peer Mapping.
pub(crate) async fn mapped_access_mode(
	f: &Federation,
	receiver: &aidash_domain::nodes::NodeName,
	source: &str,
	subject: &str,
	exclusive: bool,
) -> Result<Access> {
	let tenant = f.node_tenant_of(receiver).await?;
	let source = aidash_domain::nodes::NodeAddress::parse(source)?;
	if source.deployment == f.config.deployment && f.node_tenant_of(&source.node).await? == tenant {
		let admitted = Admitted {
			tenant,
			receiver: f.config.deployment.node(receiver.clone()),
			sender: source,
			acting: aidash_application::federation::inbound::Acting::SameTenant {
				subject: subject.into(),
			},
		};
		let mut access = access_mode(f, &admitted, exclusive).await?;
		let result = access.use_node(receiver.clone()).await;
		if let Err(error) = result {
			return access.finish::<Access>(Err(error)).await;
		}
		return Ok(access);
	}
	aidash_application::authorization::peer::mappings::mapped(
		&crate::bootstrap::peer_mapping_repository(f),
		&tenant,
		receiver,
		&source,
		subject,
		exclusive,
	)
	.await
	.map(|scope| *scope.access)
	.map_err(Into::into)
}

pub(crate) async fn access(f: &Federation, admitted: &Admitted) -> Result<Access> {
	access_mode(f, admitted, false).await
}

// Authority RPCs distinguish a permanent denial from a transport outage while
// never reflecting a peer's response body or internal error details.
//
// A peer's 404 reads as `Forbidden` here, as it always has: most callers branch on that and
// were written before the two could be told apart. A caller whose result reaches the model
// uses [`authority_request_keeping_not_found`] instead.
pub(crate) async fn authority_request<T: serde::de::DeserializeOwned>(
	f: &Federation,
	sender: &aidash_domain::federation::route::Sender,
	node: &str,
	path: &str,
	body: &serde_json::Value,
) -> Result<T> {
	fold_not_found(authority_request_keeping_not_found(f, sender, node, path, body).await)
}

/// A peer's 404 as the `Forbidden` it has always read as.
fn fold_not_found<T>(result: Result<T>) -> Result<T> {
	result.map_err(|error| match error {
		crate::Error::NotFound(_) => crate::Error::Forbidden,
		error => error,
	})
}

/// [`authority_request`] that keeps a peer's 404 as `NotFound`, for a tool's effect: the model
/// hears that the target Node did not find it instead of the Run pausing.
pub(crate) async fn authority_request_keeping_not_found<T: serde::de::DeserializeOwned>(
	f: &Federation,
	sender: &aidash_domain::federation::route::Sender,
	node: &str,
	path: &str,
	body: &serde_json::Value,
) -> Result<T> {
	crate::bootstrap::authority_peer_client(f, sender)
		.request(node, path, body)
		.await
		.map_err(Into::into)
}

pub(crate) use crate::apps::identity::serializers::peer::{
	DiscoveryInput, HistoryPage, MappingPage, MappingRevision,
};
pub use crate::apps::identity::serializers::peer::{PeerMapping, PeerMappingInput};

use crate::apps::identity::oidc::BrowserOrigin;
use crate::apps::identity::services::tenant_administration::TenantAdministrations;
use crate::authorization::identity::Actor;

#[derive(Clone)]
pub struct PeerMappings {
	pub(crate) runtime: Federation,
}

#[injectable(scope = "request")]
pub(crate) async fn provide_peer_mappings(#[inject] runtime: Federation) -> PeerMappings {
	PeerMappings { runtime }
}

impl PeerMappings {
	fn administrations(&self) -> TenantAdministrations {
		TenantAdministrations {
			runtime: self.runtime.clone(),
		}
	}
	pub(crate) async fn history(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		page: HistoryPage,
	) -> Result<Vec<MappingRevision>> {
		identifier(&tenant)?;
		crate::http::validate(&page)?;
		let pool = self.runtime.store.pool.clone();
		self.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				administrator
					.peer_mapping_history(&pool, page.after, page.limit as usize)
					.await
			})
			.await
	}
	pub(crate) async fn list(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		page: MappingPage,
	) -> Result<Vec<PeerMapping>> {
		identifier(&tenant)?;
		crate::http::validate(&page)?;
		let pool = self.runtime.store.pool.clone();
		self.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				administrator
					.peer_mappings(&pool, page.offset as usize, page.limit as usize)
					.await
			})
			.await
	}
	pub(crate) async fn set(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		input: PeerMappingInput,
	) -> Result<PeerMapping> {
		let f = self.runtime.clone();
		self.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				write_as(&f, &tenant, input, Some(administrator.clone())).await
			})
			.await
	}
	pub(crate) async fn discover(
		&self,
		admitted: Admitted,
		input: DiscoveryInput,
	) -> Result<Vec<Entry>> {
		discover_for(self.runtime.clone(), admitted, input).await
	}
	pub(crate) async fn discover_tools(
		&self,
		admitted: Admitted,
	) -> Result<aidash_domain::federation::tool_lists::ToolDiscovery> {
		tools_discover_for(self.runtime.clone(), admitted).await
	}
}

pub(crate) async fn access_mode(
	f: &Federation,
	admitted: &Admitted,
	exclusive: bool,
) -> Result<Access> {
	aidash_application::authorization::peer::mappings::access(
		&crate::bootstrap::peer_mapping_repository(f),
		admitted,
		exclusive,
	)
	.await
	.map(|scope| {
		let mut access = *scope.access;
		access.set_node(admitted.receiver.node.clone());
		access
	})
	.map_err(Into::into)
}
#[path = "peer/dependencies.rs"]
pub(crate) mod dependencies;

#[path = "peer/semantic.rs"]
pub(crate) mod semantic;

/// Discovery of another Node of this Deployment, or of another Deployment,
/// as the subject its token names.
pub(crate) async fn discover_for(
	f: Federation,
	admitted: Admitted,
	input: DiscoveryInput,
) -> Result<Vec<Entry>> {
	let mut access = access(&f, &admitted).await?;
	let result = async {
		let node = crate::apps::identity::repositories::nodes::get(
			&mut *access.tx,
			admitted.tenant.as_str(),
			admitted.receiver.node.as_str(),
		)
		.await?;
		let resource = access.resource(
			"node",
			node.name.as_str(),
			json!({"label":node.label,"default":node.default}),
		);
		access.require(&resource, "federation.discover").await?;
		let block = sender_block(&mut access, &admitted).await?;
		let mut search = input.search;
		search.kind = Some("agent".into());
		let mut visible = vec![];
		for mut entry in catalog::list_in(&mut access, &search).await? {
			if access
				.decide(&catalog::resource(&access, &entry), "agent.execute")
				.await?
			{
				if let Some(block) = &block {
					hide_blocked_tools(&mut access, &mut entry, block).await;
				}
				visible.push(entry);
			}
		}
		Ok(visible)
	}
	.await;
	access.finish(result).await
}

/// The Federation Tool Block this Node keeps for the sender, or `None` for a sender of this
/// Tenant, which registers no Node here and for which the lists do not apply.
async fn sender_block(
	access: &mut Access,
	admitted: &Admitted,
) -> Result<Option<aidash_domain::federation::tool_lists::ToolList>> {
	Ok(
		crate::apps::identity::repositories::node_registrations::get(
			&mut *access.tx,
			admitted.tenant.as_str(),
			admitted.receiver.node.as_str(),
			&admitted.sender,
		)
		.await?
		.map(|registration| registration.tool_block),
	)
}

/// Take from an Agent card the tools the receiver blocks for the sender. The card names an
/// Agent's tools in `config.bindings`, each a binding of kind `tool` that targets a definition;
/// the definition's key is what the Block matches. A definition that cannot be read or keyed
/// stays: it is only a description, and a call to it is decided again when it is made. The
/// builtins a `config.core_capabilities` flag turns on are named by the flag alone, so a flag
/// one of whose tools is blocked reads `false`.
async fn hide_blocked_tools(
	access: &mut Access,
	entry: &mut Entry,
	block: &aidash_domain::federation::tool_lists::ToolList,
) {
	use aidash_domain::federation::tool_lists::{blocked_core_capabilities, definition_key};
	if let Some(flags) = entry
		.config
		.get_mut("core_capabilities")
		.and_then(serde_json::Value::as_object_mut)
	{
		for flag in blocked_core_capabilities(block) {
			if let Some(value) = flags.get_mut(flag) {
				*value = serde_json::Value::Bool(false);
			}
		}
	}
	let Some(bindings) = entry
		.config
		.get("bindings")
		.and_then(serde_json::Value::as_array)
		.cloned()
	else {
		return;
	};
	let mut kept = Vec::with_capacity(bindings.len());
	for binding in bindings {
		if binding["kind"] == "tool"
			&& let (Some(id), Some(version)) = (
				binding["target"]["id"].as_str(),
				binding["target"]["version"].as_str(),
			) && let Ok(definition) =
			crate::apps::registry::models::Definition::read_in(&mut **access.tx, id, version).await
			&& definition_key(id, &definition.config).is_some_and(|key| block.matches(&key))
		{
			continue;
		}
		kept.push(binding);
	}
	entry.config["bindings"] = serde_json::Value::Array(kept);
}

/// The tools this Node offers `admitted.sender` to request (ADR 0139): every tool whose Reach
/// is listable, that the receiver's Allowance names and its Block does not. Between Nodes of
/// one Tenant no list applies, so every listable tool is offered. Refuses exactly as
/// [`discover_for`] does: the verified token, a mapping to a subject, and the local policy's
/// `federation.discover` decide.
pub(crate) async fn tools_discover_for(
	f: Federation,
	admitted: Admitted,
) -> Result<aidash_domain::federation::tool_lists::ToolDiscovery> {
	use aidash_domain::federation::tool_lists::{
		DiscoveredTool, RemoteToolLists, ToolDiscovery, definition_key,
	};
	use aidash_domain::tool::{ALL_BUILTINS, builtin_reach};
	let mut access = access(&f, &admitted).await?;
	let result = async {
		let node = crate::apps::identity::repositories::nodes::get(
			&mut *access.tx,
			admitted.tenant.as_str(),
			admitted.receiver.node.as_str(),
		)
		.await?;
		let resource = access.resource(
			"node",
			node.name.as_str(),
			json!({"label":node.label,"default":node.default}),
		);
		access.require(&resource, "federation.discover").await?;
		let lists = crate::apps::identity::repositories::node_registrations::get(
			&mut *access.tx,
			admitted.tenant.as_str(),
			admitted.receiver.node.as_str(),
			&admitted.sender,
		)
		.await?
		.map(|registration| (registration.tool_allowance, registration.tool_block));
		let offered = |key: &str| {
			lists.as_ref().is_none_or(|(allowance, block)| {
				RemoteToolLists::discoverable(allowance, block, key)
			})
		};
		let mut tools = vec![];
		for name in ALL_BUILTINS {
			let reach = builtin_reach(name).expect("every builtin has a Reach");
			if reach.listable() && offered(name) {
				tools.push(DiscoveredTool {
					key: (*name).to_owned(),
					reach,
				});
			}
		}
		// Registry tools the sender's authority may invoke here; a core descriptor is the
		// builtin already listed.
		let search = Search {
			kind: Some("tool".into()),
			..Search::default()
		};
		for entry in catalog::list_in(&mut access, &search).await? {
			if !access
				.decide(&catalog::resource(&access, &entry), "tool.invoke")
				.await?
			{
				continue;
			}
			let Some(key) = definition_key(&entry.id, &entry.config) else {
				continue;
			};
			if key.starts_with("registry:") && offered(&key) {
				tools.push(DiscoveredTool {
					key,
					reach: aidash_domain::tool::ToolReach::ExternalServices,
				});
			}
		}
		tools.sort_by(|left, right| left.key.cmp(&right.key));
		tools.dedup_by(|left, right| left.key == right.key);
		Ok(ToolDiscovery { tools })
	}
	.await;
	access.finish(result).await
}

#[cfg(test)]
mod authority_tests {
	use super::*;

	#[test]
	fn a_peers_404_reads_as_forbidden_and_nothing_else_changes() {
		assert!(matches!(
			fold_not_found::<()>(Err(crate::Error::NotFound(
				"the target Node did not find it".into()
			))),
			Err(crate::Error::Forbidden)
		));
		assert!(matches!(
			fold_not_found::<()>(Err(crate::Error::Conflict("changed".into()))),
			Err(crate::Error::Conflict(_))
		));
		assert_eq!(fold_not_found(Ok(7)).unwrap(), 7);
	}
}
