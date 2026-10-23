//! Management HTTP endpoints.
use crate::http::json::Json;
use crate::{authorization::identity::Actor, registry::Search};
use reinhardt::Depends;
use reinhardt::Query;
use reinhardt::Response;
use reinhardt::get;
use reinhardt::http::ViewResult;
use reinhardt::post;
use serde::Deserialize;

use crate::apps::federation::peer::services::management::PeerManagement;

/// Which Node of the caller's Tenant sends: its default Node when absent.
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SendingNode {
	pub node: Option<String>,
}

#[post("/api/discover", name = "discover", auth = "protected")]
pub async fn discover(
	#[inject] service: Depends<PeerManagement>,
	#[inject] actor: Actor,
	Query(sender): Query<SendingNode>,
	Json(query): Json<Search>,
) -> ViewResult<Response> {
	crate::http::json(service.discover(actor, sender.node, query).await)
}

#[get("/.well-known/aidash", name = "identity", auth = "public")]
pub async fn identity(#[inject] service: Depends<PeerManagement>) -> ViewResult<Response> {
	crate::http::json(service.identity().await)
}
