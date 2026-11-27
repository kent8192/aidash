//! API response contracts.
use crate::{config::DeploymentIdentity, domain::*, registry::Entry};
use schemars::JsonSchema;
use serde::Serialize;

use crate::apps::identity::serializers::session::AccessProfile;

#[derive(Serialize, JsonSchema)]
pub struct StateResponse {
	/// The current configured Offering: self-hosted, cloud or dedicated.
	pub offering: String,
	pub access: AccessProfile,
	/// The Deployment that answers. A Tenant works in one of its own Nodes, which
	/// `GET /api/tenants/{tenant}/nodes` lists.
	pub deployment: DeploymentIdentity,
	pub registry: Vec<Entry>,
	pub threads: Vec<Thread>,
	pub tasks: Vec<Task>,
	pub runs: Vec<crate::domain::RunSummary>,
	pub human_requests: Vec<HumanRequest>,
	pub conversations: Vec<Conversation>,
	pub events: Vec<Event>,
	pub partial: bool,
	pub artifacts: Vec<Artifact>,
	/// Rows this read scanned and could not confirm because a Node did not
	/// answer, for each Node, kind and Thread. `[]` when none.
	pub unconfirmed: Vec<aidash_domain::unconfirmed::Unconfirmed>,
	pub next_cursors: aidash_application::authorization::state::NextCursors,
}

#[derive(Default, serde::Deserialize, JsonSchema)]
pub struct StateQuery {
	pub node: Option<String>,
	pub events_before: Option<i64>,
}
#[derive(Default, serde::Deserialize, JsonSchema)]
pub struct ThreadPageQuery {
	pub node: Option<String>,
	pub cursor: Option<String>,
}
#[derive(Serialize, JsonSchema)]
pub struct ThreadPage {
	pub items: Vec<Thread>,
	pub next_cursor: Option<String>,
}
