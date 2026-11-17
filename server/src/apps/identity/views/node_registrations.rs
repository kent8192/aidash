//! Node Registrations of a Tenant's Nodes (#241).
use crate::apps::identity::oidc::BrowserOrigin;
use crate::apps::identity::serializers::node_registrations::{RegistrationInput, ToolListsInput};
use crate::apps::identity::serializers::peer::{HistoryPage, MappingPage};
use crate::apps::identity::serializers::tool_asks::ToolAskAnswerInput;
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

/// The tools the registered Node offers to be requested (ADR 0139).
#[get(
	"/api/tenants/{tenant}/nodes/{node}/registrations/{source_node}/tools",
	name = "tenant-node-registration-tools",
	auth = "protected"
)]
pub async fn remote_tools(
	#[inject] service: Depends<NodeRegistrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	PathStruct(RegistrationPath {
		tenant,
		node,
		source_node,
	}): PathStruct<RegistrationPath>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.remote_tools(actor, origin, tenant, node, source_node)
			.await,
	)
}

/// The calls of remote Runs that wait for this registration's list holders (ADR 0139).
#[get(
	"/api/tenants/{tenant}/nodes/{node}/registrations/{source_node}/tool-asks",
	name = "tenant-node-registration-tool-asks",
	auth = "protected"
)]
pub async fn tool_asks(
	#[inject] service: Depends<NodeRegistrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	PathStruct(RegistrationPath {
		tenant,
		node,
		source_node,
	}): PathStruct<RegistrationPath>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.tool_asks(actor, origin, tenant, node, source_node)
			.await,
	)
}

/// The path of one answer: a registration and the waiting call's admission or grant.
#[derive(serde::Deserialize)]
pub struct ToolAskPath {
	pub tenant: String,
	pub node: String,
	pub source_node: String,
	pub id: uuid::Uuid,
}

/// Answer one waiting call: this Run only, add to the list, decline, or block.
#[post(
	"/api/tenants/{tenant}/nodes/{node}/registrations/{source_node}/tool-asks/{id}/answer",
	name = "tenant-node-registration-tool-ask-answer",
	auth = "protected"
)]
pub async fn answer_tool_ask(
	#[inject] service: Depends<NodeRegistrations>,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	PathStruct(ToolAskPath {
		tenant,
		node,
		source_node,
		id,
	}): PathStruct<ToolAskPath>,
	Json(input): Json<ToolAskAnswerInput>,
) -> ViewResult<Response> {
	crate::http::json(
		service
			.answer_tool_ask(actor, origin, tenant, node, source_node, id, input)
			.await,
	)
}
