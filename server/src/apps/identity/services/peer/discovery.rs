//! Source-side discovery never falls back to the privileged legacy endpoint.
use super::super::{access::Access, catalog, identity::SubjectIdentity};
use crate::{
	Result,
	apps::federation::remote::repositories::routes,
	domain::qualified_agent,
	federation::{DiscoveredAgent, Discovery, Federation},
	registry::{Entry, Search},
};
use aidash_domain::federation::{PeerError, route::Sender};
use aidash_domain::nodes::NodeName;
use futures_util::{StreamExt, stream};
use serde_json::json;
use std::collections::BTreeSet;

pub(crate) async fn discover(
	f: &Federation,
	identity: &SubjectIdentity,
	node: Option<NodeName>,
	search: &Search,
) -> Result<Discovery> {
	let mut access = Access::begin(&f.store, identity).await?;
	let result = discover_in(f, &mut access, node, search).await;
	access.finish(result).await
}

/// Discovery from `node` of the caller's Tenant, its default Node when none is
/// named: the Nodes it addresses are asked, each as the caller's subject.
pub(crate) async fn discover_in(
	f: &Federation,
	access: &mut Access,
	node: Option<NodeName>,
	search: &Search,
) -> Result<Discovery> {
	let mut query = search.clone();
	query.kind = Some("agent".into());
	let local = catalog::list_in(access, &query).await?;
	let mut result = Discovery {
		agents: local
			.into_iter()
			.map(|entity| DiscoveredAgent {
				node_id: f.config.deployment.to_string(),
				entity,
			})
			.collect(),
		errors: vec![],
	};
	let sender = Sender {
		tenant: aidash_domain::tenancy::TenantName::new(access.identity.tenant.clone())?,
		node,
		subject: Some(access.identity.subject.clone()),
	};
	// Configuration changes wait for this admitted read: the routes are held
	// with a shared lock until the authority transaction ends.
	let routes = routes::all(&mut *access.tx, &f.config.deployment, &sender, true).await?;
	if let Some(route) = routes.first() {
		// Sending from a Node is working in it.
		let sending = access.resource("node", route.sender.node.as_str(), json!({}));
		access
			.require(
				&sending,
				aidash_domain::identity::tenant_administration::NODE_USE,
			)
			.await?;
	}
	let mut targets = vec![];
	for route in routes {
		let target = route.target.to_string();
		let resource = access.resource("node", &target, json!({"remote_node":target}));
		if access.decide(&resource, "federation.discover").await? {
			targets.push(target);
		}
	}
	let body = json!({"search":query});
	let mut responses = stream::iter(targets.into_iter().map(|target| {
		let body = &body;
		let sender = &sender;
		async move {
			let response = f
				.request::<Vec<Entry>>(
					sender,
					&target,
					reqwest::Method::POST,
					"/scoped/discover",
					Some(body),
				)
				.await;
			(target, response)
		}
	}))
	.buffer_unordered(8);
	while let Some((node, response)) = responses.next().await {
		let entries = match response {
			Ok(entries) => entries,
			Err(_) => {
				result.errors.push(PeerError {
					node_id: node,
					error: "authorized peer discovery unavailable".into(),
				});
				continue;
			}
		};
		// Validate a whole peer response before admitting any of its records.
		// Untrusted metadata cannot select another node's resource namespace.
		let mut ids = BTreeSet::new();
		if entries.len() > 1024
			|| entries.iter().any(|entry| {
				entry.kind != "agent"
					|| crate::registry::validate(entry).is_err()
					|| !ids.insert((&entry.id, &entry.version))
			}) {
			result.errors.push(PeerError {
				node_id: node,
				error: "invalid peer discovery response".into(),
			});
			continue;
		}
		for entity in entries.into_iter().filter(|entry| query.matches(entry)) {
			let mut resource = catalog::resource(access, &entity);
			resource.id = qualified_agent(&node, &entity.id, &entity.version);
			resource.attributes["remote_node"] = json!(node);
			if access.decide(&resource, "registry.read").await?
				&& access.decide(&resource, "agent.execute").await?
			{
				result.agents.push(DiscoveredAgent {
					node_id: node.clone(),
					entity,
				});
			}
		}
	}
	result.agents.sort_by(|left, right| {
		(&left.node_id, &left.entity.id, &left.entity.version).cmp(&(
			&right.node_id,
			&right.entity.id,
			&right.entity.version,
		))
	});
	result
		.errors
		.sort_by(|left, right| left.node_id.cmp(&right.node_id));
	access.track_discovery(&result.agents).await?;
	Ok(result)
}
