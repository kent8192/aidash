//! API response contracts.
use crate::{config::NodeIdentity, domain::*, federation::Peer, registry::Entry};
use schemars::JsonSchema;
use serde::Serialize;

use crate::apps::identity::serializers::session::AccessProfile;
use crate::apps::registry::serializers::installations::Installation;

#[derive(Serialize, JsonSchema)]
pub struct StateResponse {
	pub access: AccessProfile,
	pub node: NodeIdentity,
	pub registry: Vec<Entry>,
	pub workspaces: Vec<Workspace>,
	pub tasks: Vec<Task>,
	pub runs: Vec<crate::domain::RunInspection>,
	pub human_requests: Vec<HumanRequest>,
	pub conversations: Vec<Conversation>,
	pub peers: Vec<Peer>,
	pub events: Vec<Event>,
	pub artifacts: Vec<Artifact>,
	pub installations: Vec<Installation>,
}
