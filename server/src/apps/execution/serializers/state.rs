//! API response contracts.
use crate::{config::DeploymentIdentity, domain::*, registry::Entry};
use schemars::JsonSchema;
use serde::Serialize;

use crate::apps::identity::serializers::session::AccessProfile;

#[derive(Serialize, JsonSchema)]
pub struct StateResponse {
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
}
