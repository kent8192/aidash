//! Native working rows and business sessions have independent codecs.
use crate::apps::execution::capabilities::serializers::contracts::{
	Area as NativeArea, SessionStatus as NativeStatus,
};
use aidash_domain::capabilities::sessions::{Area, SessionStatus};
impl From<NativeArea> for Area {
	fn from(value: NativeArea) -> Self {
		Self {
			id: value.id,
			tenant: value.tenant,
			home_node: value.home_node,
			thread_id: value.thread_id,
			agent_id: value.agent_id,
			owner: value.owner,
			generation: value.generation,
			revision: value.revision,
			epoch: value.epoch,
			state: value.state,
			manifest: serde_json::json!([]),
			constraints: value.constraints,
		}
	}
}
impl From<&NativeArea> for Area {
	fn from(value: &NativeArea) -> Self {
		Self {
			id: value.id,
			tenant: value.tenant.clone(),
			home_node: value.home_node.clone(),
			thread_id: value.thread_id,
			agent_id: value.agent_id.clone(),
			owner: value.owner.clone(),
			generation: value.generation,
			revision: value.revision,
			epoch: value.epoch,
			state: value.state.clone(),
			manifest: serde_json::json!([]),
			constraints: value.constraints.clone(),
		}
	}
}
impl From<Area> for NativeArea {
	fn from(value: Area) -> Self {
		Self {
			id: value.id,
			tenant: value.tenant,
			home_node: value.home_node,
			thread_id: value.thread_id,
			agent_id: value.agent_id,
			owner: value.owner,
			generation: value.generation,
			revision: value.revision,
			epoch: value.epoch,
			state: value.state,
			constraints: value.constraints,
		}
	}
}
impl From<SessionStatus> for NativeStatus {
	fn from(value: SessionStatus) -> Self {
		Self {
			area_id: value.area_id,
			last_run_id: value.last_run_id,
			last_agent_version: value.last_agent_version,
			active_run_id: value.active_run_id,
		}
	}
}
