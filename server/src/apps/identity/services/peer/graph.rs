//! Scoped graph projection. A peer connection authenticates the source Node;
//! a mapped Subject or a typed operator grant supplies the receiving authority.
pub(crate) use crate::apps::identity::repositories::graph::persistence::{
	GraphAuthority, candidate_visible, candidates, decode_cursor, encode_cursor, graph_generation,
	linked_registry, linked_task, linked_workspace, list_grants, operator_grant, project_activity,
	set_grant, source_peer_lease,
};

use crate::{
	Error, Result,
	authorization::{access::Access, identity::Actor},
	federation::Federation,
};
use http::HeaderMap;
use serde_json::json;

pub(crate) fn grant_page_size() -> u64 {
	100
}

pub(crate) async fn peers(
	f: Federation,
	actor: Actor,
	origin: Option<crate::dashboard_auth::BrowserOrigin>,
) -> Result<Vec<GraphPeer>> {
	let peers = f.peers().await?;
	match actor {
		Actor::Subject(identity) => {
			let mut access = Access::begin(&f.store, &identity).await?;
			let result = async {
				let mut visible = Vec::new();
				for peer in peers.into_iter().filter(|peer| peer.enabled) {
					let resource =
						access.resource("node", &peer.node_id, json!({"remote_node":peer.node_id}));
					if access.decide(&resource, "federation.graph.read").await? {
						visible.push(GraphPeer {
							node_id: peer.node_id,
						});
					}
				}
				Ok(visible)
			}
			.await;
			access.finish(result).await
		}
		Actor::Operator if origin.is_some() => Ok(peers
			.into_iter()
			.filter(|peer| peer.enabled)
			.map(|peer| GraphPeer {
				node_id: peer.node_id,
			})
			.collect()),
		Actor::Operator => Err(Error::Forbidden),
	}
}

pub(crate) async fn expand(
	f: Federation,
	actor: Actor,
	origin: Option<crate::dashboard_auth::BrowserOrigin>,
	input: GraphExpandInput,
) -> Result<GraphPage> {
	input.options.validate()?;
	crate::config::validate_node_id(&input.node_id)?;
	match actor {
		Actor::Subject(identity) => {
			if input.options.target_tenant.is_some() {
				return Err(Error::Forbidden);
			}
			let mut access = Access::begin(&f.store, &identity).await?;
			let result = async {
				let resource =
					access.resource("node", &input.node_id, json!({"remote_node":input.node_id}));
				access.require(&resource, "federation.graph.read").await?;
				if let Some(id) = input.options.scope_workspace {
					access.require_workspace(id, "workspace.read").await?;
				}
				source_peer_lease(&mut access.tx, &input.node_id).await?;
				remote_page(
					&f,
					&input,
					GraphViewer::Subject {
						tenant: identity.tenant,
						subject: identity.subject,
					},
				)
				.await
			}
			.await;
			access.finish(result).await
		}
		Actor::Operator => {
			let id = origin.ok_or(Error::Forbidden)?.identity_id;
			if input.options.target_tenant.is_none() {
				return Err(Error::Invalid("target tenant required".into()));
			}
			let mut tx = f.store.pool.begin().await?;
			source_peer_lease(&mut tx, &input.node_id).await?;
			let result = remote_page(&f, &input, GraphViewer::Operator { id }).await;
			if result.is_ok() {
				tx.commit().await?;
			} else {
				tx.rollback().await?;
			}
			result
		}
	}
}

fn valid_remote_page(page: &GraphPage, input: &GraphExpandInput) -> bool {
	aidash_domain::federation::graph::valid_remote_page(page, &input.node_id, &input.options)
}

async fn remote_page(
	f: &Federation,
	input: &GraphExpandInput,
	viewer: GraphViewer,
) -> Result<GraphPage> {
	let request = GraphRequest {
		viewer,
		options: input.options.clone(),
	};
	let response = f
		.peer_response(
			&input.node_id,
			reqwest::Method::POST,
			"/scoped/graph",
			Some(&serde_json::to_value(request)?),
		)
		.await
		.map_err(|_| Error::External("remote graph unavailable".into()))?;
	let status = response.status().as_u16();
	let page: GraphPage = match status {
		200 => crate::response::json(response, 4_194_304)
			.await
			.map_err(|_| Error::External("invalid remote graph response".into()))?,
		401 | 403 => return Err(Error::Forbidden),
		400 => {
			return Err(Error::Invalid(
				"remote graph resource exceeds page limit".into(),
			));
		}
		404 => return Err(Error::NotFound("remote graph feature unavailable".into())),
		409 => return Err(Error::Conflict("remote graph generation changed".into())),
		_ => return Err(Error::External("remote graph unavailable".into())),
	};
	if !valid_remote_page(&page, input) {
		return Err(Error::External("invalid remote graph response".into()));
	}
	Ok(page)
}

pub(crate) async fn project(
	f: Federation,
	headers: HeaderMap,
	input: GraphRequest,
) -> Result<GraphPage> {
	input.options.validate()?;
	let node = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	let page = project_page(&f, node, input).await?;
	Ok(page)
}

async fn project_in(
	f: &Federation,
	source_node: &str,
	viewer: &GraphViewer,
	options: &GraphOptions,
	authority: &mut GraphAuthority<'_>,
	authority_revision: &str,
) -> Result<GraphPage> {
	aidash_application::federation::graph::project(
		&mut crate::bootstrap::graph_projection_scope(
			f,
			authority,
			source_node,
			authority_revision,
		),
		source_node,
		viewer,
		options,
	)
	.await
	.map_err(Into::into)
}

async fn project_page(f: &Federation, node: &str, input: GraphRequest) -> Result<GraphPage> {
	match &input.viewer {
		GraphViewer::Subject { tenant, subject } => {
			if input.options.target_tenant.is_some() {
				return Err(Error::Forbidden);
			}
			let mut access = super::access(f, node, tenant, subject).await?;
			let result = async {
				let resource = access.resource("node", &f.config.node_id, json!({}));
				access.require(&resource, "federation.graph.read").await?;
				let mapping_revision =
					crate::apps::identity::repositories::graph::persistence::mapping_revision(
						&mut access,
						node,
						tenant,
						subject,
					)
					.await?;
				let revision = format!(
					"{}:{}:{}",
					access.snapshot.revision, access.identity.credential_id, mapping_revision
				);
				project_in(
					f,
					node,
					&input.viewer,
					&input.options,
					&mut GraphAuthority::Subject(&mut access),
					&revision,
				)
				.await
			}
			.await;
			access.finish(result).await
		}
		GraphViewer::Operator { id } => {
			let tenant = input
				.options
				.target_tenant
				.as_deref()
				.ok_or(Error::Forbidden)?;
			let (mut tx, grant) = operator_grant(f, node, *id, tenant).await?;
			let revision = format!("{}:{}", grant.source_operator, grant.revision);
			let result = project_in(
				f,
				node,
				&input.viewer,
				&input.options,
				&mut GraphAuthority::Operator {
					tenant,
					tx: &mut tx,
				},
				&revision,
			)
			.await;
			if result.is_ok() {
				tx.commit().await?;
			} else {
				tx.rollback().await?;
			}
			result
		}
	}
}

#[derive(Clone)]
pub struct GraphManagement {
	pub(crate) runtime: Federation,
}
// Preserve a statement before the value until reinhardt-web#6441 is fixed.
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> GraphManagement {
	tracing::trace!(service = "GraphManagement", "creating injectable service");
	GraphManagement { runtime }
}

use reinhardt::injectable;

use reinhardt::Query as QueryParams;

pub use crate::apps::identity::serializers::peer_graph::{
	GraphActivity, GraphExpandInput, GraphNode, GraphOperatorGrant, GraphOperatorGrantInput,
	GraphOptions, GraphPage, GraphPeer, GraphRequest,
};

pub(crate) use crate::apps::identity::serializers::peer_graph::{
	GrantPage, GraphCursor, GraphViewer,
};

#[cfg(test)]
#[path = "../../tests/services_peer_graph_tests.rs"]
mod tests;

#[cfg(test)]
use chrono::Utc;

#[cfg(test)]
use std::collections::BTreeMap;

#[cfg(test)]
use crate::apps::identity::serializers::peer_graph::GraphEdge;
#[cfg(test)]
use aidash_domain::federation::graph::entity_key;
