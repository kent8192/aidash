//! Scoped graph projection. A peer connection authenticates the source Node;
//! a mapped Subject supplies the receiving authority.
pub(crate) use crate::apps::identity::repositories::graph::persistence::{
	GraphAuthority, source_peer_lease,
};

use crate::{
	Error, Result,
	apps::federation::remote::repositories::routes,
	authorization::{access::Access, identity::Actor},
	federation::Federation,
};
use aidash_application::federation::inbound::Admitted;
use aidash_domain::federation::route::Sender;
use serde_json::json;

/// The Sender for work the caller's Tenant does from `node` (its default Node
/// when none is named).
fn sender(
	identity: &crate::authorization::identity::SubjectIdentity,
	node: Option<&str>,
	acting: bool,
) -> Result<Sender> {
	Ok(Sender {
		tenant: aidash_domain::tenancy::TenantName::new(identity.tenant.clone())?,
		node: node
			.map(aidash_domain::nodes::NodeName::parse)
			.transpose()?,
		subject: acting.then(|| identity.subject.clone()),
	})
}

pub(crate) async fn peers(
	f: Federation,
	actor: Actor,
	node: Option<String>,
) -> Result<Vec<GraphPeer>> {
	match actor {
		Actor::Subject(identity) => {
			let sender = sender(&identity, node.as_deref(), true)?;
			let mut access = Access::begin(&f.store, &identity).await?;
			let result = async {
				let sending = routes::sending_node(&mut *access.tx, &sender).await?;
				access.use_node(sending).await?;
				let routes =
					routes::all(&mut *access.tx, &f.config.deployment, &sender, false).await?;
				let mut visible = Vec::new();
				for route in routes {
					let target = route.target.to_string();
					let resource = access.resource("node", &target, json!({"remote_node":target}));
					if access.decide(&resource, "federation.graph.read").await? {
						visible.push(GraphPeer { node_id: target });
					}
				}
				Ok(visible)
			}
			.await;
			access.finish(result).await
		}
		// Cross-Tenant Graph View needs a Subject; a Deployment Administrator
		// has none and gets no Tenant content (#486).
		Actor::DeploymentAdministrator => Err(Error::Forbidden),
	}
}

pub(crate) async fn expand(
	f: Federation,
	actor: Actor,
	input: GraphExpandInput,
) -> Result<GraphPage> {
	input.options.validate()?;
	match actor {
		Actor::Subject(identity) => {
			aidash_domain::nodes::NodeAddress::parse(&input.node_id)?;
			let sender = sender(&identity, input.sending_node.as_deref(), true)?;
			let mut access = Access::begin(&f.store, &identity).await?;
			let result = async {
				let sending = routes::sending_node(&mut *access.tx, &sender).await?;
				access.use_node(sending).await?;
				let resource =
					access.resource("node", &input.node_id, json!({"remote_node":input.node_id}));
				access.require(&resource, "federation.graph.read").await?;
				if let Some(id) = input.options.scope_workspace {
					access.require_workspace(id, "workspace.read").await?;
				}
				source_peer_lease(
					&mut access.tx,
					&f.config.deployment,
					&sender,
					&input.node_id,
				)
				.await?;
				remote_page(&f, &input, identity.subject, &sender).await
			}
			.await;
			access.finish(result).await
		}
		Actor::DeploymentAdministrator => Err(Error::Forbidden),
	}
}

fn valid_remote_page(page: &GraphPage, input: &GraphExpandInput) -> bool {
	aidash_domain::federation::graph::valid_remote_page(page, &input.node_id, &input.options)
}

async fn remote_page(
	f: &Federation,
	input: &GraphExpandInput,
	subject: String,
	sender: &Sender,
) -> Result<GraphPage> {
	let request = GraphRequest {
		subject,
		options: input.options.clone(),
	};
	let body = serde_json::to_value(request)?;
	let response = f
		.peer_response(
			sender,
			&input.node_id,
			reqwest::Method::POST,
			"/scoped/graph",
			Some(&body),
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
	admitted: Admitted,
	input: GraphRequest,
) -> Result<GraphPage> {
	input.options.validate()?;
	project_page(&f, admitted, input).await
}

async fn project_in(
	f: &Federation,
	source_node: &str,
	receiver: &str,
	subject: &str,
	options: &GraphOptions,
	authority: &mut GraphAuthority<'_>,
	authority_revision: &str,
) -> Result<GraphPage> {
	aidash_application::federation::graph::project(
		&mut crate::bootstrap::graph_projection_scope(
			f,
			authority,
			source_node,
			receiver,
			authority_revision,
		),
		source_node,
		subject,
		options,
	)
	.await
	.map_err(Into::into)
}

async fn project_page(
	f: &Federation,
	admitted: Admitted,
	input: GraphRequest,
) -> Result<GraphPage> {
	// The token names the subject; the body only repeats it.
	if admitted.subject() != Some(input.subject.as_str()) {
		return Err(Error::Forbidden);
	}
	let node = admitted.sender.to_string();
	let mut access = super::access(f, &admitted).await?;
	let result = async {
		let receiving = crate::apps::identity::repositories::nodes::get(
			&mut *access.tx,
			admitted.tenant.as_str(),
			admitted.receiver.node.as_str(),
		)
		.await?;
		let resource = access.resource(
			"node",
			receiving.name.as_str(),
			json!({"label":receiving.label,"default":receiving.default}),
		);
		access.require(&resource, "federation.graph.read").await?;
		let mapping_revision =
			crate::apps::identity::repositories::graph::persistence::mapping_revision(
				&mut access,
				&admitted,
				&input.subject,
			)
			.await?;
		let revision = format!(
			"{}:{}:{}",
			access.snapshot.revision, access.identity.credential_id, mapping_revision
		);
		project_in(
			f,
			&node,
			&admitted.receiver.to_string(),
			&input.subject,
			&input.options,
			&mut GraphAuthority(&mut access),
			&revision,
		)
		.await
	}
	.await;
	access.finish(result).await
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

pub use crate::apps::identity::serializers::peer_graph::{
	GraphExpandInput, GraphOptions, GraphPage, GraphPeer, GraphRequest,
};

#[cfg(test)]
#[path = "../../tests/services_peer_graph_tests.rs"]
mod tests;

#[cfg(test)]
use chrono::Utc;

#[cfg(test)]
use std::collections::BTreeMap;

#[cfg(test)]
use crate::apps::identity::serializers::peer_graph::{GraphEdge, GraphNode};
#[cfg(test)]
use aidash_domain::federation::graph::entity_key;
