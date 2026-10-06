//! Runtime tool declarations. Names are assigned here; consumers evaluate attributes.
use super::ToolConfig;
use crate::{
	capabilities::CoreCapabilities,
	registry::{AgentConfig, EntityRef},
};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolEffect {
	/// Addressed user resources remain unchanged; audit receipts are allowed.
	ReadOnly,
	/// Repeating the same invocation key does not duplicate its effect.
	Idempotent,
	/// No replay guarantee; severity and approval are separate declarations.
	Unsafe,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolUseMode {
	Ordinary,
	MessageCatchUp,
	MediaPending,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisclosureBoundary {
	Local,
	Home,
	External,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Continuation {
	Ordinary,
	Human,
	Wait,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultFitting {
	WorkspaceRecord,
	SkillText,
	Observation,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentFlag {
	TaskCreation,
	Delegation,
	MemoryWrite,
	MemoryRead,
	WorkspaceRetrieval,
}
impl AgentFlag {
	pub fn permitted(self, config: &AgentConfig) -> bool {
		match self {
			Self::TaskCreation => config.allow_task_creation != Some(false),
			Self::Delegation => config.allow_task_delegation != Some(false),
			Self::MemoryWrite => config.memory.is_some() && config.allow_memory_write == Some(true),
			Self::MemoryRead => {
				config.memory.is_some() && config.allow_cross_conversation_memory != Some(false)
			}
			Self::WorkspaceRetrieval => config.allow_workspace_retrieval != Some(false),
		}
	}
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CorePermission {
	Files,
	Shell,
	Python,
	Patch,
	Skills,
	Sharing,
	Outbound,
}
impl CorePermission {
	pub fn permitted(self, config: &CoreCapabilities) -> bool {
		match self {
			Self::Files => config.files,
			Self::Shell => config.shell,
			Self::Python => config.python,
			Self::Patch => config.patch,
			Self::Skills => config.skills,
			Self::Sharing => config.sharing,
			Self::Outbound => config.shell || config.python,
		}
	}
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolIdentity {
	Builtin(String),
	Registry(EntityRef),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceTarget {
	Workspace,
	Task,
	Agent,
	Run,
	Argument(&'static str),
	ArgumentUuid(&'static str),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthorizationRequirement {
	Resource {
		action: &'static str,
		kind: &'static str,
		target: ResourceTarget,
	},
	LocalDelegation,
	ConfiguredSkill,
	AgentExecution {
		node_id: String,
		agent: EntityRef,
	},
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolAuthorization {
	pub flag: Option<AgentFlag>,
	pub core: Option<CorePermission>,
	pub requirements: Vec<AuthorizationRequirement>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolBehavior {
	pub effect: ToolEffect,
	pub fitting: Option<ResultFitting>,
	pub continuation: Continuation,
	pub catch_up: bool,
	pub media_pending: bool,
	pub yields_model_media: bool,
	pub workbench_approval: bool,
}
impl ToolBehavior {
	pub fn permits(&self, mode: ToolUseMode) -> bool {
		match mode {
			ToolUseMode::Ordinary => true,
			ToolUseMode::MessageCatchUp => self.catch_up,
			ToolUseMode::MediaPending => self.media_pending,
		}
	}
	pub fn fitting_for(&self, input: &Value) -> Option<ResultFitting> {
		match self.fitting {
			Some(ResultFitting::SkillText) if input.get("skill").is_none() => None,
			other => other,
		}
	}
	pub fn model_media_result(&self, input: &Value, output: &Value) -> bool {
		self.yields_model_media
			&& input["representation"] == "model_input"
			&& output["status"] == "completed"
			&& output["metadata"]["file_id"] == input["file_id"]
	}
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolContract {
	pub identity: ToolIdentity,
	pub authorization: ToolAuthorization,
	pub behavior: ToolBehavior,
	pub disclosure: DisclosureBoundary,
	pub remote_exposure: bool,
}
impl ToolContract {
	pub fn replay_safe(&self) -> bool {
		self.behavior.effect != ToolEffect::Unsafe
	}
	pub fn registry(reference: EntityRef, config: &ToolConfig) -> Self {
		let (effect, approval, disclosure) = match config {
			ToolConfig::Http { replay, .. } | ToolConfig::Mcp { replay, .. } => (
				match replay.as_str() {
					"read_only" => ToolEffect::ReadOnly,
					"idempotent" => ToolEffect::Idempotent,
					_ => ToolEffect::Unsafe,
				},
				replay != "read_only",
				DisclosureBoundary::External,
			),
			ToolConfig::Agent { .. } => (ToolEffect::Idempotent, false, DisclosureBoundary::Home),
			ToolConfig::Native { operation, .. } => (
				ToolEffect::ReadOnly,
				false,
				if operation == "http_get" {
					DisclosureBoundary::External
				} else {
					DisclosureBoundary::Local
				},
			),
		};
		Self {
			identity: ToolIdentity::Registry(reference),
			authorization: ToolAuthorization {
				flag: None,
				core: None,
				requirements: if let ToolConfig::Agent { node_id, agent } = config {
					vec![AuthorizationRequirement::AgentExecution {
						node_id: node_id.clone(),
						agent: agent.clone(),
					}]
				} else {
					vec![]
				},
			},
			behavior: ToolBehavior {
				effect,
				workbench_approval: approval,
				fitting: None,
				continuation: Continuation::Ordinary,
				catch_up: false,
				media_pending: false,
				yields_model_media: false,
			},
			disclosure,
			remote_exposure: true,
		}
	}
}

/// The sole builtin assignment table. Grant identity is independent of model alias.
pub fn builtin_contract(name: &str) -> Option<ToolContract> {
	use AuthorizationRequirement::*;
	use ResourceTarget::*;
	let mut contract = ToolContract {
		identity: ToolIdentity::Builtin(format!("builtin:{name}")),
		authorization: ToolAuthorization {
			flag: None,
			core: None,
			requirements: vec![],
		},
		behavior: ToolBehavior {
			effect: ToolEffect::Idempotent,
			fitting: None,
			continuation: Continuation::Ordinary,
			catch_up: false,
			media_pending: false,
			yields_model_media: false,
			workbench_approval: false,
		},
		disclosure: DisclosureBoundary::Home,
		remote_exposure: false,
	};
	let auth = &mut contract.authorization;
	let behavior = &mut contract.behavior;
	let resource = |action, kind, target| Resource {
		action,
		kind,
		target,
	};
	match name {
		"task_create" => {
			auth.flag = Some(AgentFlag::TaskCreation);
			auth.requirements
				.push(resource("task.create", "workspace", Workspace));
			contract.remote_exposure = true;
		}
		"task_assign" => {
			auth.flag = Some(AgentFlag::Delegation);
			auth.requirements.push(resource(
				"generation.request",
				"generation_policy",
				Argument("policy_id"),
			));
		}
		"task_delegate" => {
			auth.flag = Some(AgentFlag::Delegation);
			auth.requirements = vec![
				LocalDelegation,
				resource("task.delegate", "task", ArgumentUuid("task_id")),
			];
		}
		"artifact_publish" => {
			auth.requirements
				.push(resource("artifact.create", "artifact", Task));
			contract.remote_exposure = true;
		}
		"workspace_message" => {
			auth.requirements
				.push(resource("message.create", "workspace", Workspace));
			contract.remote_exposure = true;
		}
		"memory_mutate" => {
			auth.flag = Some(AgentFlag::MemoryWrite);
			auth.requirements
				.push(resource("memory.write", "memory", Agent));
		}
		"memory_recall" | "memory_reflect" => {
			auth.flag = Some(AgentFlag::MemoryRead);
			auth.requirements.push(resource(
				if name == "memory_recall" {
					"memory.read"
				} else {
					"memory.reflect"
				},
				"memory",
				Agent,
			));
			behavior.effect = ToolEffect::ReadOnly;
		}
		"human_request" => {
			auth.requirements
				.push(resource("human.request", "run", Run));
			behavior.continuation = Continuation::Human;
		}
		"agent_discover" => {
			behavior.effect = ToolEffect::ReadOnly;
		}
		"workspace_read" | "workspace_observe" | "workspace_wait" => {
			auth.flag = Some(AgentFlag::WorkspaceRetrieval);
			contract.remote_exposure = true;
			behavior.effect = ToolEffect::ReadOnly;
			match name {
				"workspace_read" => {
					behavior.fitting = Some(ResultFitting::WorkspaceRecord);
					behavior.catch_up = true;
					behavior.media_pending = true;
				}
				"workspace_observe" => {
					behavior.fitting = Some(ResultFitting::Observation);
					behavior.media_pending = true;
				}
				_ => behavior.continuation = Continuation::Wait,
			}
		}
		"skill_read" => {
			auth.core = Some(CorePermission::Skills);
			auth.requirements.push(ConfiguredSkill);
			behavior.effect = ToolEffect::ReadOnly;
			behavior.fitting = Some(ResultFitting::SkillText);
			behavior.media_pending = true;
			contract.remote_exposure = true;
		}
		"skill_list" | "skill_load" => {
			auth.core = Some(CorePermission::Skills);
			behavior.effect = ToolEffect::ReadOnly;
			behavior.media_pending = name == "skill_list";
			contract.disclosure = DisclosureBoundary::Local;
		}
		"file_read" | "file_search" => {
			auth.core = Some(CorePermission::Files);
			behavior.effect = ToolEffect::ReadOnly;
			behavior.media_pending = true;
			behavior.yields_model_media = name == "file_read";
			contract.disclosure = DisclosureBoundary::Local;
		}
		"shell" | "shell_poll" | "shell_cancel" => {
			auth.core = Some(CorePermission::Shell);
			contract.disclosure = DisclosureBoundary::Local;
		}
		"code_interpreter" | "python_install" | "python_poll" | "python_cancel" => {
			auth.core = Some(CorePermission::Python);
			contract.disclosure = DisclosureBoundary::Local;
		}
		"apply_patch" => {
			auth.core = Some(CorePermission::Patch);
			contract.disclosure = DisclosureBoundary::Local;
		}
		"file_share" => {
			auth.core = Some(CorePermission::Sharing);
		}
		"outbound_get" => {
			auth.core = Some(CorePermission::Outbound);
			behavior.effect = ToolEffect::ReadOnly;
			contract.disclosure = DisclosureBoundary::External;
		}
		_ => return None,
	}
	Some(contract)
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn native_memory_contracts_require_explicit_policy_and_stay_at_home() {
		let mut config: AgentConfig = serde_json::from_value(serde_json::json!({
			"model": {"id":"model", "version":"1"}
		}))
		.unwrap();
		assert!(!AgentFlag::MemoryWrite.permitted(&config));
		assert!(!AgentFlag::MemoryRead.permitted(&config));
		config.memory = Some(EntityRef {
			id: "memory".into(),
			version: "1".into(),
		});
		assert!(AgentFlag::MemoryRead.permitted(&config));
		assert!(!AgentFlag::MemoryWrite.permitted(&config));
		config.allow_memory_write = Some(true);
		assert!(AgentFlag::MemoryWrite.permitted(&config));
		config.allow_cross_conversation_memory = Some(false);
		assert!(!AgentFlag::MemoryRead.permitted(&config));
		for (name, action, effect) in [
			("memory_mutate", "memory.write", ToolEffect::Idempotent),
			("memory_recall", "memory.read", ToolEffect::ReadOnly),
			("memory_reflect", "memory.reflect", ToolEffect::ReadOnly),
		] {
			let contract = builtin_contract(name).unwrap();
			assert_eq!(contract.disclosure, DisclosureBoundary::Home);
			assert!(!contract.remote_exposure);
			assert!(contract.replay_safe());
			assert_eq!(contract.behavior.effect, effect);
			assert_eq!(
				contract.authorization.requirements,
				vec![AuthorizationRequirement::Resource {
					action,
					kind: "memory",
					target: ResourceTarget::Agent
				}]
			);
		}
		assert!(builtin_contract("memory_write").is_none());
	}
	#[test]
	fn modes_and_remote_exposure_are_explicit_sets() {
		let names = [
			"task_create",
			"task_assign",
			"task_delegate",
			"artifact_publish",
			"workspace_message",
			"memory_mutate",
			"memory_recall",
			"memory_reflect",
			"human_request",
			"agent_discover",
			"workspace_read",
			"workspace_observe",
			"workspace_wait",
			"skill_read",
			"skill_list",
			"skill_load",
			"file_read",
			"file_search",
			"shell",
			"shell_poll",
			"shell_cancel",
			"code_interpreter",
			"python_install",
			"python_poll",
			"python_cancel",
			"apply_patch",
			"file_share",
			"outbound_get",
		];
		let selected = |predicate: fn(&ToolContract) -> bool| {
			names
				.iter()
				.copied()
				.filter(|name| predicate(&builtin_contract(name).unwrap()))
				.collect::<Vec<_>>()
		};
		assert_eq!(
			selected(|c| c.remote_exposure),
			[
				"task_create",
				"artifact_publish",
				"workspace_message",
				"workspace_read",
				"workspace_observe",
				"workspace_wait",
				"skill_read"
			]
		);
		assert_eq!(
			selected(|c| c.behavior.permits(ToolUseMode::MessageCatchUp)),
			["workspace_read"]
		);
		assert_eq!(
			selected(|c| c.behavior.permits(ToolUseMode::MediaPending)),
			[
				"workspace_read",
				"workspace_observe",
				"skill_read",
				"skill_list",
				"file_read",
				"file_search"
			]
		);
		assert!(
			!builtin_contract("agent_discover")
				.unwrap()
				.behavior
				.media_pending
		);
		assert_eq!(
			builtin_contract("outbound_get").unwrap().disclosure,
			DisclosureBoundary::External
		);
	}
	#[test]
	fn registry_effects_and_approval_are_independent() {
		let reference = EntityRef {
			id: "tool".into(),
			version: "1".into(),
		};
		for (replay, effect, approval, safe) in [
			("read_only", ToolEffect::ReadOnly, false, true),
			("idempotent", ToolEffect::Idempotent, true, true),
			("unsafe", ToolEffect::Unsafe, true, false),
		] {
			let contract = ToolContract::registry(
				reference.clone(),
				&ToolConfig::Http {
					endpoint: "https://example.invalid".into(),
					credential_env: None,
					replay: replay.into(),
				},
			);
			assert_eq!(contract.identity, ToolIdentity::Registry(reference.clone()));
			assert_eq!(contract.behavior.effect, effect);
			assert_eq!(contract.behavior.workbench_approval, approval);
			assert_eq!(contract.replay_safe(), safe);
			assert!(contract.remote_exposure);
		}
	}
}
