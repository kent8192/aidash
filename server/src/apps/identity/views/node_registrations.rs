//! Node Registrations of a Tenant's Nodes (#241).
use crate::apps::identity::oidc::BrowserOrigin;
use crate::apps::identity::serializers::node_registrations::{RegistrationInput, ToolListsInput};
use crate::apps::identity::serializers::peer::{HistoryPage, MappingPage};
use crate::apps::identity::services::node_registrations::NodeRegistrations;
use crate::authorization::identity::Actor;
use crate::http::json::Json;
use reinhardt::di::params::PathStruct;
use reinhardt::{Depends, Path, Query, Response, get, http::ViewResult, post, put};

#[get(
	"/api/tenants/{tenant}/nodes/{node}/registrations",
	name = "tenant-node-registrations",
	auth = "protected"
)]
pub async fn list(
	#[inject] service: Depends<NodeRegistrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, node)): Path<(String, String)>,
	Query(page): Query<MappingPage>,
) -> ViewResult<Response> {
	crate::http::json(service.list(actor, origin, tenant, node, page).await)
}

#[get(
	"/api/tenants/{tenant}/nodes/{node}/registration-history",
	name = "tenant-node-registration-history",
	auth = "protected"
)]
pub async fn history(
	#[inject] service: Depends<NodeRegistrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, node)): Path<(String, String)>,
	Query(page): Query<HistoryPage>,
) -> ViewResult<Response> {
	crate::http::json(service.history(actor, origin, tenant, node, page).await)
}

#[post(
	"/api/tenants/{tenant}/nodes/{node}/registrations",
	name = "tenant-node-registration-set",
	auth = "protected"
)]
pub async fn set(
	#[inject] service: Depends<NodeRegistrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path((tenant, node)): Path<(String, String)>,
	Json(input): Json<RegistrationInput>,
) -> ViewResult<Response> {
	crate::http::json(service.set(actor, origin, tenant, node, input).await)
}

/// The three path segments of one registration; tuple extraction stops at two strings.
#[derive(serde::Deserialize)]
pub struct RegistrationPath {
	pub tenant: String,
	pub node: String,
	pub source_node: String,
}

#[put(
	"/api/tenants/{tenant}/nodes/{node}/registrations/{source_node}/tool-lists",
	name = "tenant-node-registration-tool-lists",
	auth = "protected"
)]
pub async fn set_tool_lists(
	#[inject] service: Depends<NodeRegistrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	PathStruct(RegistrationPath {
		tenant,
		node,
		source_node,
	}): PathStruct<RegistrationPath>,
	Json(input): Json<ToolListsInput>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.set_tool_lists(actor, origin, tenant, node, source_node, input)
			.await,
	)
}
