//! Typed response envelopes shared by axum, utoipa and the generated client.
use crate::{
    config::NodeIdentity,
    domain::*,
    federation::{Delegation, Peer},
    registry::Entry,
    store::Invocation,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AccessProfile {
    Operator,
    Subject { tenant: String, subject: String },
}

#[derive(Serialize, ToSchema)]
pub struct SessionResponse {
    pub access: AccessProfile,
    pub node_id: String,
}

#[derive(Serialize, ToSchema)]
pub struct StateResponse {
    pub access: AccessProfile,
    pub node: NodeIdentity,
    pub registry: Vec<Entry>,
    pub workspaces: Vec<Workspace>,
    pub tasks: Vec<Task>,
    pub runs: Vec<Run>,
    pub human_requests: Vec<HumanRequest>,
    pub conversations: Vec<Conversation>,
    pub peers: Vec<Peer>,
    pub events: Vec<Event>,
    pub artifacts: Vec<Artifact>,
    pub installations: Vec<Installation>,
}
#[derive(Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct Installation {
    pub id: String,
    pub version: String,
    pub digest: String,
    pub config: Value,
    pub installed_at: chrono::DateTime<chrono::Utc>,
}
#[derive(Serialize, ToSchema)]
pub struct ConversationResponse {
    pub conversation: Conversation,
    pub workspace: Workspace,
    pub task: Task,
    pub delegation: Delegation,
}
#[derive(Serialize, ToSchema)]
pub struct RunDetails {
    pub run: Run,
    pub invocations: Vec<Invocation>,
    pub memory: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PeerError {
    pub node_id: String,
    pub error: String,
}
#[derive(Serialize, Deserialize, ToSchema)]
pub struct MeshNode {
    pub node_id: String,
    pub runs: Vec<Run>,
    pub human_requests: Vec<HumanRequest>,
    pub invocations: Vec<Invocation>,
}
#[derive(Serialize, ToSchema)]
pub struct MeshResponse {
    pub nodes: Vec<MeshNode>,
    pub errors: Vec<PeerError>,
}
#[derive(Serialize, ToSchema)]
pub struct SentResponse {
    pub sent: bool,
}
