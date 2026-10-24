//! Native working rows and business sessions have independent codecs.
use crate::apps::execution::capabilities::serializers::contracts::{
	Area as NativeArea, SessionRun as NativeRun, SessionStatus as NativeStatus,
};
use aidash_domain::capabilities::sessions::{Area, SessionRun, SessionStatus};
impl From<NativeArea> for Area {
	fn from(value: NativeArea) -> Self {
		Self {
			id: value.id,
			tenant: value.tenant,
			home_node: value.home_node,
			workspace_id: value.workspace_id,
			thread_id: value.thread_id,
			agent_id: value.agent_id,
			owner: value.owner,
			generation: value.generation,
			revision: value.revision,
			epoch: value.epoch,
			state: value.state,
			manifest: value.manifest,
			constraints: value.constraints,
			next_sequence: value.next_sequence,
		}
	}
}
impl From<&NativeArea> for Area {
	fn from(value: &NativeArea) -> Self {
		Self {
			id: value.id,
			tenant: value.tenant.clone(),
			home_node: value.home_node.clone(),
			workspace_id: value.workspace_id,
			thread_id: value.thread_id,
			agent_id: value.agent_id.clone(),
			owner: value.owner.clone(),
			generation: value.generation,
			revision: value.revision,
			epoch: value.epoch,
			state: value.state.clone(),
			manifest: value.manifest.clone(),
			constraints: value.constraints.clone(),
			next_sequence: value.next_sequence,
		}
	}
}
impl From<Area> for NativeArea {
	fn from(value: Area) -> Self {
		Self {
			id: value.id,
			tenant: value.tenant,
			home_node: value.home_node,
			workspace_id: value.workspace_id,
			thread_id: value.thread_id,
			agent_id: value.agent_id,
			owner: value.owner,
			generation: value.generation,
			revision: value.revision,
			epoch: value.epoch,
			state: value.state,
			manifest: value.manifest,
			constraints: value.constraints,
			next_sequence: value.next_sequence,
		}
	}
}
impl From<NativeRun> for SessionRun {
	fn from(value: NativeRun) -> Self {
		Self {
			run_id: value.run_id,
			sequence: value.sequence,
			phase: value.phase,
			control: value.control,
		}
	}
}
impl From<SessionRun> for NativeRun {
	fn from(value: SessionRun) -> Self {
		Self {
			run_id: value.run_id,
			sequence: value.sequence,
			phase: value.phase,
			control: value.control,
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
			queue: value.queue.into_iter().map(Into::into).collect(),
		}
	}
}
