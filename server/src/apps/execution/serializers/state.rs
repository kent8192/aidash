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
	pub workspaces: Vec<Workspace>,
	pub tasks: Vec<Task>,
	pub runs: Vec<crate::domain::RunInspection>,
	pub human_requests: Vec<HumanRequest>,
	pub conversations: Vec<Conversation>,
	pub events: Vec<Event>,
	pub artifacts: Vec<Artifact>,
	pub next_cursors: aidash_application::authorization::state::NextCursors,
}

#[derive(Default, serde::Deserialize, JsonSchema)]
pub struct StateQuery {
	pub node: Option<String>,
}
#[derive(Default, serde::Deserialize, JsonSchema)]
pub struct WorkspacePageQuery {
	pub node: Option<String>,
	pub cursor: Option<String>,
}
#[derive(Serialize, JsonSchema)]
pub struct WorkspacePage {
	pub items: Vec<Workspace>,
	pub next_cursor: Option<String>,
}
