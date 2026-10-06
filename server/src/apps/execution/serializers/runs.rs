//! API response contracts.
use crate::{
	domain::{HumanRequest, RunInspection},
	store::Invocation,
};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize, schemars::JsonSchema)]
pub struct RunDetails {
	pub run: crate::domain::RunInspection,
	pub invocations: Vec<Invocation>,
	pub memory: Value,
	/// Each current route lists MIME types accepted together by the run model.
	pub media_input_routes: Vec<Vec<String>>,
}

#[derive(Serialize, JsonSchema)]
pub struct PeerObservation {
	pub node_id: String,
	pub runs: Vec<RunInspection>,
	pub human_requests: Vec<HumanRequest>,
	pub invocations: Vec<Invocation>,
}
