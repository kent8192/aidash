//! API response contracts.
use crate::{domain::*, store::Invocation};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, schemars::JsonSchema)]
pub struct MeshNode {
	pub node_id: String,
	pub runs: Vec<crate::domain::RunInspection>,
	pub human_requests: Vec<HumanRequest>,
	pub invocations: Vec<Invocation>,
}

#[derive(Serialize, JsonSchema)]
pub struct MeshResponse {
	pub nodes: Vec<MeshNode>,
	pub errors: Vec<PeerError>,
}

pub use aidash_domain::federation::PeerError;
