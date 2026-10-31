//! Runtime tool declarations. Names are assigned here; consumers evaluate attributes.
use super::{Concurrency, ToolConfig};
use crate::{
	agent_plugin::{PluginName, ServerName},
	capabilities::CoreCapabilities,
	registry::{AgentConfig, EntityRef},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ToolEffect {
	/// Addressed user resources remain unchanged; audit receipts are allowed.
	ReadOnly,
	/// Repeating the same invocation key does not duplicate its effect.
	Idempotent,
	/// No replay guarantee; severity and approval are separate declarations.
	Unsafe,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ToolUseMode {
	Ordinary,
	MessageCatchUp,
	MediaPending,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum DisclosureBoundary {
	Local,
	Home,
	External,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Continuation {
	Ordinary,
	Human,
	Wait,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ResultFitting {
	WorkspaceRecord,
	SkillText,
	/// A `skill_asset_read` text page: `content` from `offset` to `next_offset`.
	SkillAsset,
	/// A `capability_describe` result, useful only complete: recorded whole
	/// when it fits the remaining request budget, otherwise deferred.
	CapabilityDescription,
	/// A `capability_search` page, cut to its leading results that fit the
	/// remaining request budget; `next_cursor` resumes after the last kept one.
	CapabilitySearch,
	Observation,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
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
			Self::Outbound => config.outbound || config.shell || config.python,
		}
	}
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum ToolIdentity {
	Builtin(String),
	Registry(EntityRef),
	Descriptor(crate::registry::bindings::QualifiedRef),
	AgentPlugin(AgentPluginTool),
}
/// A tool of a remote MCP server that a Run pinned from an Agent Plugin copy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPluginTool {
	pub plugin: PluginName,
	/// The [`crate::agent_plugin::AgentPluginRef::digest`] of the copy.
	pub digest: String,
	pub server: ServerName,
	/// The server's own name for the tool.
	pub tool: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ResourceTarget {
	Workspace,
	Task,
	Agent,
	Run,
	Argument(&'static str),
	ArgumentUuid(&'static str),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ToolAuthorization {
	pub flag: Option<AgentFlag>,
	pub core: Option<CorePermission>,
	pub requirements: Vec<AuthorizationRequirement>,
}
impl ToolAuthorization {
	fn none() -> Self {
		Self {
			flag: None,
			core: None,
			requirements: vec![],
		}
	}
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ToolBehavior {
	pub effect: ToolEffect,
	pub fitting: Option<ResultFitting>,
	pub continuation: Continuation,
	pub catch_up: bool,
	pub media_pending: bool,
	pub yields_model_media: bool,
	/// A call waits for a Tool call approval unless policy permits it directly.
	pub tool_call_approval: bool,
	/// A declared Read-only operation of a Tenant's HTTP or MCP tool. Its calls
	/// need no Tool call approval, yet the Tenant's policy may still forbid
	/// them outright (ADR 0076).
	#[serde(skip_serializing_if = "std::ops::Not::not")]
	pub read_only_operation: bool,
	/// Omitted while Sequential so contracts pinned before this field keep their digest.
	#[serde(skip_serializing_if = "Concurrency::is_sequential")]
	pub concurrency: Concurrency,
	/// The output's `"exposure_update"` changes the Run's Exposure set.
	#[serde(skip_serializing_if = "std::ops::Not::not")]
	pub exposure_update: bool,
	/// Research contract result kinds this operation yields. A Run that binds
	/// any `Citations` operation has its final answer's Source citations checked.
	#[serde(skip_serializing_if = "Vec::is_empty")]
	pub yields: Vec<Yield>,
	/// The provider sends Run-derived content outside the Node only after a
	/// Web disclosure approval; the Run waits on it like any approval.
	#[serde(skip_serializing_if = "std::ops::Not::not")]
	pub disclosure_approval: bool,
}

/// Result kinds of the host-owned research contract, declared per operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Yield {
	/// Source records from Web search; discovery only, never evidence.
	Search,
	/// A Document snapshot from Page reading.
	Fetch,
	/// Evidence fragments that a final answer may cite.
	Citations,
}
impl ToolBehavior {
	/// A call of a Tenant's HTTP or MCP tool, whose result is external data: it
	/// passes the Tool call gate, which asks or only checks, and it can never
	/// raise an approval of its own.
	pub fn tenant_tool_call(&self) -> bool {
		self.tool_call_approval || self.read_only_operation
	}
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
	pub fn yields(&self, kind: Yield) -> bool {
		self.yields.contains(&kind)
	}
	pub fn model_media_result(&self, input: &Value, output: &Value) -> bool {
		self.model_media_selection(input, output).is_some()
	}
	/// The model input a completed call selects: a Working Area file read as
	/// model input, or the page image of an Evidence fragment (ADR 0053).
	pub fn model_media_selection(
		&self,
		input: &Value,
		output: &Value,
	) -> Option<crate::media::ModelMedia> {
		use crate::media::{ModelFileSelection, ModelMedia, PageImageSelection};
		if !self.yields_model_media {
			return None;
		}
		if input["representation"] == "model_input"
			&& output["status"] == "completed"
			&& output["metadata"]["file_id"] == input["file_id"]
		{
			return serde_json::from_value::<ModelFileSelection>(serde_json::json!({
				"file_id": input["file_id"],
				"expected_digest": input["expected_digest"],
				"workspace_id": input["workspace_id"],
				"area_id": input["area_id"],
			}))
			.ok()
			.map(ModelMedia::File);
		}
		let data = &output["data"];
		(output["status"] == "ok")
			.then(|| {
				Some(ModelMedia::PageImage(PageImageSelection {
					evidence_ref: data["evidence_ref"].as_str()?.to_owned(),
					expected_digest: data["image"]["digest"].as_str()?.to_owned(),
				}))
			})
			.flatten()
	}
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
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
	/// The digest that pins this provider contract into a Run's Binding snapshot.
	pub fn digest(&self) -> crate::Result<String> {
		Ok(crate::registry::rules::digest(&serde_json::to_value(self)?))
	}
	/// The contract a Run pinned under `digest`, if any. A Run admitted before
	/// its provider declared Concurrency safety keeps that operation Sequential;
	/// a legacy Read-only definition keeps its approval-gated Unsafe contract.
	pub fn pinned(mut self, digest: &str) -> crate::Result<Option<Self>> {
		if self.digest()? == digest {
			return Ok(Some(self));
		}
		if !self.behavior.concurrency.is_sequential() {
			self.behavior.concurrency = Concurrency::Sequential;
			if self.digest()? == digest {
				return Ok(Some(self));
			}
		}
		// Existing HTTP/MCP versions could already declare `replay: read_only`
		// before that declaration changed their effect and approval gate. Only
		// restore this older contract when the Run pinned its exact digest.
		if matches!(self.identity, ToolIdentity::Registry(_)) && self.behavior.read_only_operation {
			self.behavior.effect = ToolEffect::Unsafe;
			self.behavior.tool_call_approval = true;
			self.behavior.read_only_operation = false;
			if self.digest()? == digest {
				return Ok(Some(self));
			}
		}
		Ok(None)
	}
	/// A pinned plugin tool is Unsafe, Sequential and never exposed remotely. Declaring it
	/// Read-only removes the approval only; the effect stays Unsafe like a Tenant tool's.
	pub fn agent_plugin(identity: AgentPluginTool, read_only: bool) -> Self {
		Self {
			identity: ToolIdentity::AgentPlugin(identity),
			authorization: ToolAuthorization::none(),
			behavior: ToolBehavior {
				// A read-only tool the Agent version declares still passes the
				// Tool call gate, so an explicit deny refuses it (ADR 0076).
				read_only_operation: read_only,
				..external_behavior(ToolEffect::Unsafe, !read_only)
			},
			disclosure: DisclosureBoundary::External,
			remote_exposure: false,
		}
	}
	pub fn registry(reference: EntityRef, config: &ToolConfig) -> Self {
		let (effect, approval, disclosure) = match config {
			// A Read-only operation still sends its arguments to the registered
			// service, so disclosure stays External; Concurrency stays Sequential.
			ToolConfig::Http { .. } | ToolConfig::Mcp { .. } if config.declares_read_only() => {
				(ToolEffect::ReadOnly, false, DisclosureBoundary::External)
			}
			ToolConfig::Http { replay, .. } | ToolConfig::Mcp { replay, .. }
				if replay == "idempotent" =>
			{
				(ToolEffect::Idempotent, true, DisclosureBoundary::External)
			}
			ToolConfig::Http { .. } | ToolConfig::Mcp { .. } => {
				(ToolEffect::Unsafe, true, DisclosureBoundary::External)
			}
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
		let read_only_operation = config.declares_read_only();
		Self {
			identity: ToolIdentity::Registry(reference),
			authorization: ToolAuthorization {
				requirements: if let ToolConfig::Agent { node_id, agent } = config {
					vec![AuthorizationRequirement::AgentExecution {
						node_id: node_id.clone(),
						agent: agent.clone(),
					}]
				} else {
					vec![]
				},
				..ToolAuthorization::none()
			},
			behavior: ToolBehavior {
				read_only_operation,
				..external_behavior(effect, approval)
			},
			disclosure,
			remote_exposure: true,
		}
	}
}

/// The behavior of an ordinary tool that no model media, catch-up or yield attributes touch.
fn external_behavior(effect: ToolEffect, tool_call_approval: bool) -> ToolBehavior {
	ToolBehavior {
		effect,
		tool_call_approval,
		fitting: None,
		continuation: Continuation::Ordinary,
		catch_up: false,
		media_pending: false,
		yields_model_media: false,
		read_only_operation: false,
		concurrency: Concurrency::Sequential,
		exposure_update: false,
		yields: vec![],
		disclosure_approval: false,
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
			tool_call_approval: false,
			read_only_operation: false,
			concurrency: Concurrency::Sequential,
			exposure_update: false,
			yields: vec![],
			disclosure_approval: false,
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
		// A Remote-Grant-started creator needs to manage its receiving-Node
		// Subnode; each effect rechecks its creator or member identity.
		"subnode_create"
		| "subnode_task"
		| "subnode_add_member"
		| "subnode_remove_member"
		| "subnode_dissolve" => {
			contract.remote_exposure = true;
		}
		"subnode_message" => {}
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
		"skill_asset_read" => {
			auth.core = Some(CorePermission::Skills);
			auth.requirements.push(ConfiguredSkill);
			behavior.effect = ToolEffect::ReadOnly;
			behavior.fitting = Some(ResultFitting::SkillAsset);
			behavior.media_pending = true;
			contract.disclosure = DisclosureBoundary::Local;
			contract.remote_exposure = true;
		}
		"capability_search" | "capability_describe" => {
			behavior.effect = ToolEffect::ReadOnly;
			behavior.media_pending = true;
			contract.disclosure = DisclosureBoundary::Local;
			behavior.fitting = Some(if name == "capability_describe" {
				ResultFitting::CapabilityDescription
			} else {
				ResultFitting::CapabilitySearch
			});
			contract.remote_exposure = true;
		}
		"capability_load" | "capability_unload" => {
			behavior.exposure_update = true;
			contract.disclosure = DisclosureBoundary::Local;
			contract.remote_exposure = true;
		}
		"skill_list" | "skill_load" => {
			auth.core = Some(CorePermission::Skills);
			behavior.effect = ToolEffect::ReadOnly;
			behavior.media_pending = name == "skill_list";
			behavior.concurrency = Concurrency::SharedRead;
			contract.disclosure = DisclosureBoundary::Local;
		}
		"file_read" | "file_search" => {
			auth.core = Some(CorePermission::Files);
			behavior.effect = ToolEffect::ReadOnly;
			behavior.media_pending = true;
			behavior.yields_model_media = name == "file_read";
			behavior.concurrency = Concurrency::SharedRead;
			contract.disclosure = DisclosureBoundary::Local;
		}
		"shell" | "shell_poll" | "shell_cancel" => {
			auth.core = Some(CorePermission::Shell);
			contract.disclosure = DisclosureBoundary::Local;
		}
		"code_interpreter" | "python_poll" | "python_cancel" => {
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
		"live_preview_start" | "live_preview_stop" => {
			// A Live preview runs a Sandboxed command under the shell permission;
			// it changes state outside the model's files.
			auth.core = Some(CorePermission::Shell);
			behavior.effect = ToolEffect::Unsafe;
			contract.disclosure = DisclosureBoundary::Local;
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
			"schema_version":1, "instructions":"Use native memory", "remove_default":["memory_mutate"], "model": {"id":"model", "version":"1.0.0"}
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
			"python_poll",
			"python_cancel",
			"apply_patch",
			"file_share",
			"outbound_get",
			"capability_search",
			"capability_describe",
			"capability_load",
			"capability_unload",
			"skill_asset_read",
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
				"skill_read",
				"capability_search",
				"capability_describe",
				"capability_load",
				"capability_unload",
				"skill_asset_read"
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
				"file_search",
				"capability_search",
				"capability_describe",
				"skill_asset_read"
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
		assert_eq!(
			selected(|c| c.behavior.exposure_update),
			["capability_load", "capability_unload"]
		);
		for name in [
			"capability_search",
			"capability_describe",
			"capability_load",
			"capability_unload",
		] {
			let contract = builtin_contract(name).unwrap();
			assert_eq!(contract.disclosure, DisclosureBoundary::Local);
			assert_eq!(contract.authorization.flag, None);
			assert_eq!(contract.authorization.core, None);
			assert!(contract.authorization.requirements.is_empty());
			assert_eq!(
				contract.behavior.effect,
				if contract.behavior.exposure_update {
					ToolEffect::Idempotent
				} else {
					ToolEffect::ReadOnly
				}
			);
		}
		assert_eq!(
			selected(|c| c.behavior.fitting == Some(ResultFitting::CapabilityDescription)),
			["capability_describe"]
		);
		assert_eq!(
			selected(|c| c.behavior.fitting == Some(ResultFitting::CapabilitySearch)),
			["capability_search"]
		);
		let asset = builtin_contract("skill_asset_read").unwrap();
		assert_eq!(asset.behavior.effect, ToolEffect::ReadOnly);
		assert_eq!(asset.disclosure, DisclosureBoundary::Local);
		assert_eq!(asset.authorization.core, Some(CorePermission::Skills));
		assert_eq!(asset.behavior.fitting, Some(ResultFitting::SkillAsset));
		assert_eq!(
			asset.authorization.requirements,
			[AuthorizationRequirement::ConfiguredSkill]
		);
	}
	#[test]
	fn existing_builtin_contracts_serialize_without_exposure_fields() {
		// Every pinned `provider_contract_digest` hashes this exact behavior shape.
		let legacy_keys = [
			"effect",
			"fitting",
			"continuation",
			"catch_up",
			"media_pending",
			"yields_model_media",
			"tool_call_approval",
		];
		for name in [
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
			"python_poll",
			"python_cancel",
			"apply_patch",
			"file_share",
			"outbound_get",
		] {
			let contract = builtin_contract(name).unwrap();
			let encoded = serde_json::to_string(&contract).unwrap();
			assert!(!encoded.contains("exposure_update"), "{name}");
			let behavior = &encoded[encoded.find(r#""behavior":{"#).unwrap()..];
			let behavior = &behavior[..behavior.find('}').unwrap()];
			let keys = legacy_keys
				.iter()
				.map(|key| behavior.find(&format!(r#""{key}":"#)).unwrap())
				.collect::<Vec<_>>();
			assert!(keys.windows(2).all(|pair| pair[0] < pair[1]), "{name}");
			// Shared-read builtins also encode their concurrency; `pinned` keeps
			// Runs admitted before that declaration on their Sequential digest.
			let concurrent = usize::from(!contract.behavior.concurrency.is_sequential());
			assert_eq!(
				behavior.contains(r#""concurrency":"shared_read""#),
				concurrent == 1,
				"{name}"
			);
			assert_eq!(
				behavior.matches("\":").count(),
				legacy_keys.len() + 1 + concurrent,
				"{name}"
			);
		}
		let encoded = serde_json::to_value(builtin_contract("capability_load").unwrap()).unwrap();
		assert_eq!(encoded["behavior"]["exposure_update"], true);
	}
	#[test]
	fn subnode_model_media_keeps_the_area_needed_for_authorization() {
		let file = uuid::Uuid::new_v4();
		let workspace = uuid::Uuid::new_v4();
		let area = uuid::Uuid::new_v4();
		let input = serde_json::json!({"file_id":file,"expected_digest":"sha256:fixture","representation":"model_input","workspace_id":workspace,"area_id":area});
		let output = serde_json::json!({"status":"completed","metadata":{"file_id":file}});
		let selected = builtin_contract("file_read")
			.unwrap()
			.behavior
			.model_media_selection(&input, &output)
			.unwrap();
		assert_eq!(
			serde_json::to_value(selected).unwrap(),
			serde_json::json!({"file_id":file,"expected_digest":"sha256:fixture","workspace_id":workspace,"area_id":area})
		);
		let old_input = serde_json::json!({"file_id":file,"expected_digest":"sha256:fixture","representation":"model_input"});
		let old = builtin_contract("file_read")
			.unwrap()
			.behavior
			.model_media_selection(&old_input, &output)
			.unwrap();
		assert_eq!(
			serde_json::to_value(old).unwrap(),
			serde_json::json!({"file_id":file,"expected_digest":"sha256:fixture"})
		);
	}
	#[test]
	fn research_attributes_leave_admitted_builtin_digests_unchanged() {
		// Admitted Runs pin `provider_contract_digest` over these bytes; the
		// research `yields` and `disclosure_approval` attributes are omitted
		// when empty, so existing contracts keep their digests.
		let contract = builtin_contract("workspace_read").unwrap();
		assert_eq!(
			serde_json::to_string(&contract.behavior).unwrap(),
			r#"{"effect":"ReadOnly","fitting":"WorkspaceRecord","continuation":"Ordinary","catch_up":true,"media_pending":true,"yields_model_media":false,"tool_call_approval":false}"#
		);
		assert_eq!(
			crate::registry::rules::digest(&serde_json::to_value(&contract).unwrap()),
			PINNED_WORKSPACE_READ_DIGEST
		);
	}
	fn plugin_tool() -> AgentPluginTool {
		AgentPluginTool {
			plugin: PluginName::try_from("acme-tools".to_owned()).unwrap(),
			digest: format!("sha256:{}", "a".repeat(64)),
			server: ServerName::try_from("docs".to_owned()).unwrap(),
			tool: "search".into(),
		}
	}
	#[test]
	fn a_plugin_tool_is_unsafe_sequential_external_and_never_exposed_remotely() {
		for read_only in [false, true] {
			let contract = ToolContract::agent_plugin(plugin_tool(), read_only);
			assert_eq!(contract.identity, ToolIdentity::AgentPlugin(plugin_tool()));
			assert_eq!(contract.behavior.effect, ToolEffect::Unsafe);
			assert!(!contract.replay_safe());
			assert_eq!(contract.behavior.tool_call_approval, !read_only);
			assert_eq!(contract.behavior.read_only_operation, read_only);
			assert!(contract.behavior.tenant_tool_call());
			assert!(contract.behavior.concurrency.is_sequential());
			assert_eq!(contract.disclosure, DisclosureBoundary::External);
			assert!(!contract.remote_exposure);
			assert_eq!(contract.authorization, ToolAuthorization::none());
		}
	}
	#[test]
	fn a_read_only_plugin_tool_differs_from_a_normal_one_in_its_approval_gate_only() {
		let normal = ToolContract::agent_plugin(plugin_tool(), false);
		let mut read_only = ToolContract::agent_plugin(plugin_tool(), true);

		assert_ne!(normal.digest().unwrap(), read_only.digest().unwrap());
		assert!(normal.behavior.tool_call_approval);
		assert!(!read_only.behavior.tool_call_approval);
		assert!(read_only.behavior.read_only_operation);
		read_only.behavior.tool_call_approval = true;
		read_only.behavior.read_only_operation = false;
		assert_eq!(normal, read_only);
	}
	#[test]
	fn a_plugin_tool_behaves_like_a_registry_mcp_tool_apart_from_identity_and_exposure() {
		let registry = ToolContract::registry(
			EntityRef {
				id: "tool".into(),
				version: "1".into(),
			},
			&ToolConfig::Mcp {
				secret: None,
				endpoint: "https://example.invalid".into(),
				tool_name: "search".into(),
				replay: "unsafe".into(),
				idempotency_argument: None,
			},
		);
		let plugin = ToolContract::agent_plugin(plugin_tool(), false);

		assert_eq!(plugin.behavior, registry.behavior);
		assert_eq!(plugin.authorization, registry.authorization);
		assert_eq!(plugin.disclosure, registry.disclosure);
		assert!(registry.remote_exposure);
	}
	#[test]
	fn only_a_tenants_http_or_mcp_tool_passes_the_tool_call_gate() {
		let reference = EntityRef {
			id: "tool".into(),
			version: "1".into(),
		};
		for config in [
			ToolConfig::Agent {
				node_id: "node".into(),
				agent: reference.clone(),
			},
			ToolConfig::Native {
				operation: "http_get".into(),
				allowed_hosts: vec![],
			},
		] {
			let contract = ToolContract::registry(reference.clone(), &config);
			assert!(!contract.behavior.tenant_tool_call());
		}
		assert!(
			!builtin_contract("human_request")
				.unwrap()
				.behavior
				.tenant_tool_call()
		);
	}
	const PINNED_WORKSPACE_READ_DIGEST: &str =
		"sha256:b170d3d1156fa2b6e7a52d332864282afa6b5c3b742597a1ca9833520f4ada4e";
	#[test]
	fn admitted_read_only_definitions_keep_their_approval_gated_contract() {
		// Persisted contract shape before Read-only declarations changed approval
		// behavior. Both HTTP and MCP definitions pinned these same bytes.
		let legacy = serde_json::json!({
			"identity": {"Registry": {"id": "tool", "version": "1"}},
			"authorization": {"flag": null, "core": null, "requirements": []},
			"behavior": {
				"effect": "Unsafe", "fitting": null, "continuation": "Ordinary",
				"catch_up": false, "media_pending": false, "yields_model_media": false,
				"tool_call_approval": true
			},
			"disclosure": "External", "remote_exposure": true
		});
		let digest = crate::registry::rules::digest(&legacy);
		for config in [
			ToolConfig::Http {
				endpoint: "https://example.invalid".into(),
				replay: "read_only".into(),
				secret: None,
			},
			ToolConfig::Mcp {
				endpoint: "https://example.invalid".into(),
				secret: None,
				tool_name: "lookup".into(),
				replay: "read_only".into(),
				idempotency_argument: None,
			},
		] {
			let current = ToolContract::registry(
				EntityRef {
					id: "tool".into(),
					version: "1".into(),
				},
				&config,
			);
			let pinned = current
				.clone()
				.pinned(&digest)
				.unwrap()
				.expect("the admitted Run keeps its contract");
			assert_eq!(serde_json::to_value(pinned).unwrap(), legacy);
			assert_eq!(
				current.clone().pinned(&current.digest().unwrap()).unwrap(),
				Some(current.clone())
			);
			assert!(current.pinned("sha256:unknown").unwrap().is_none());
		}
	}
	#[test]
	fn only_a_declared_read_only_operation_needs_no_tool_call_approval() {
		let reference = EntityRef {
			id: "tool".into(),
			version: "1".into(),
		};
		// Only read-only skips approval; idempotence separately permits replay.
		for (replay, effect, approval, safe) in [
			("read_only", ToolEffect::ReadOnly, false, true),
			("idempotent", ToolEffect::Idempotent, true, true),
			("unsafe", ToolEffect::Unsafe, true, false),
			("READ_ONLY", ToolEffect::Unsafe, true, false),
			("", ToolEffect::Unsafe, true, false),
		] {
			for config in [
				ToolConfig::Http {
					endpoint: "https://example.invalid".into(),
					replay: replay.into(),
					secret: None,
				},
				ToolConfig::Mcp {
					endpoint: "https://example.invalid".into(),
					secret: None,
					tool_name: "lookup".into(),
					replay: replay.into(),
					idempotency_argument: None,
				},
			] {
				let contract = ToolContract::registry(reference.clone(), &config);
				assert_eq!(contract.identity, ToolIdentity::Registry(reference.clone()));
				assert_eq!(contract.behavior.effect, effect, "{replay:?}");
				assert_eq!(contract.behavior.tool_call_approval, approval, "{replay:?}");
				// Either way the call passes the Tool call gate: asked, or only
				// checked against the Tenant's policy.
				assert_eq!(
					contract.behavior.read_only_operation,
					effect == ToolEffect::ReadOnly,
					"{replay:?}"
				);
				assert!(contract.behavior.tenant_tool_call(), "{replay:?}");
				assert_eq!(contract.replay_safe(), safe, "{replay:?}");
				assert_eq!(contract.disclosure, DisclosureBoundary::External);
				assert!(contract.remote_exposure);
				// A Read-only operation implies no Concurrency safety.
				assert!(contract.behavior.concurrency.is_sequential(), "{replay:?}");
				assert!(
					!super::super::concurrency::batchable(&contract.behavior),
					"{replay:?}"
				);
			}
		}
	}
}
