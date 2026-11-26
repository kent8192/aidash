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
	Operation,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ResultFitting {
	/// Serialized under its name before #1008: admitted Runs pin the contract
	/// digest over these bytes.
	#[serde(rename = "WorkspaceRecord")]
	ThreadRecord,
	SkillText,
	JournalText,
	JournalSearch,
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
	Thread,
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
	/// A connection consent wait remains available under every Approval Mode.
	#[serde(skip_serializing_if = "std::ops::Not::not")]
	pub connection_request: bool,
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
	/// The call may change the Tenant's policy, an Authority Change (ADR 0121),
	/// such as creating a Node, which copies Agent subjects into the policy. The
	/// Run releases its own authority lease while the call runs, so the change
	/// never waits on the Run that asked for it (#1012). Omitted when false so
	/// pinned contracts keep their digest.
	#[serde(skip_serializing_if = "std::ops::Not::not")]
	pub authority_change: bool,
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
			Some(ResultFitting::ThreadRecord) if input["kind"] == "attachment" => None,
			Some(ResultFitting::SkillText) if input.get("skill").is_none() => None,
			Some(ResultFitting::ThreadRecord) if input["kind"] == "file" => None,
			other => other,
		}
	}
	pub fn model_media_result(&self, input: &Value, output: &Value) -> bool {
		self.model_media_selection(input, output).is_some()
	}
	/// The model input selected by a completed file or Thread Attachment read.
	pub fn model_media_selection(
		&self,
		input: &Value,
		output: &Value,
	) -> Option<crate::media::ModelMedia> {
		use crate::media::{ModelFileSelection, ModelMedia, WorkingImageSelection};
		if !(self.yields_model_media
			|| self.fitting == Some(ResultFitting::ThreadRecord) && input["kind"] == "attachment")
		{
			return None;
		}
		if input["kind"] == "file"
			&& input["representation"] == "model_input"
			&& output["status"] == "completed"
			&& output["metadata"]["file_id"] == input["id"]
			&& output["metadata"]["digest"] == input["expected_digest"]
			&& output["metadata"]["path"] == input["path"]
		{
			return serde_json::from_value::<WorkingImageSelection>(serde_json::json!({
				"file_id": input["id"], "area_id": input["area_id"], "generation": input["generation"],
				"path": input["path"], "expected_digest": input["expected_digest"],
			}))
			.ok()
			.map(ModelMedia::WorkingImage);
		}
		// `workspace_read` also yields model media for Working Area images, so an
		// attachment input (no `file_id`) must fall through to its own branch
		// instead of matching null against null here.
		if self.yields_model_media
			&& input["representation"] == "model_input"
			&& output["status"] == "completed"
			&& !input["file_id"].is_null()
			&& output["metadata"]["file_id"] == input["file_id"]
		{
			return serde_json::from_value::<ModelFileSelection>(serde_json::json!({
				"file_id": input["file_id"],
				"expected_digest": input["expected_digest"],
				// `workspace_read`, which saved Agents pin, names it `workspace_id`.
				"thread_id": if input["thread_id"].is_null() {
					input["workspace_id"].clone()
				} else {
					input["thread_id"].clone()
				},
				"area_id": input["area_id"],
			}))
			.ok()
			.map(ModelMedia::File);
		}
		if input["kind"] == "attachment"
			&& input["representation"] == "model_input"
			&& output["status"] == "completed"
		{
			let id = serde_json::from_value::<uuid::Uuid>(input["id"].clone()).ok()?;
			let metadata_id =
				serde_json::from_value::<uuid::Uuid>(output["metadata"]["id"].clone()).ok()?;
			let selection: crate::media::ThreadAttachmentSelection =
				serde_json::from_value(output["selection"].clone()).ok()?;
			return (id == metadata_id && id == selection.attachment_id)
				.then_some(ModelMedia::ThreadAttachment(selection));
		}
		None
	}
}
/// What a tool touches; decides whether a Run started by a Remote Grant could use it (ADR 0139).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolReach {
	/// The Run itself: always usable by a remote Run, never listed.
	Run,
	HomeWorkspace,
	ReceiverMaterial,
	ReceiverAgents,
	People,
	ExternalServices,
	Execution,
	/// Never offered to a remote Run.
	Never,
}
impl ToolReach {
	/// Whether a Federation Tool Request / Allowance may list a tool of this Reach.
	pub fn listable(self) -> bool {
		!matches!(self, Self::Run | Self::Never)
	}
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ToolContract {
	/// The model provider executes this capability inside inference.
	#[serde(skip_serializing_if = "std::ops::Not::not")]
	pub provider_owned: bool,
	pub identity: ToolIdentity,
	pub authorization: ToolAuthorization,
	pub behavior: ToolBehavior,
	pub disclosure: DisclosureBoundary,
	/// What the tool touches; a remote Run may use only what its Reach allows (ADR 0139).
	pub reach: ToolReach,
}
impl ToolContract {
	pub fn replay_safe(&self) -> bool {
		self.behavior.effect != ToolEffect::Unsafe
	}
	/// The digest that pins this provider contract into a Run's Binding snapshot.
	pub fn digest(&self) -> crate::Result<String> {
		Ok(crate::registry::rules::digest(&serde_json::to_value(self)?))
	}
	/// A remote Run without mounted Skills keeps its existing default exclusions.
	pub fn legacy_skill_reach(&self) -> Option<Self> {
		let legacy_skill = match &self.identity {
			ToolIdentity::Builtin(name) => {
				matches!(name.as_str(), "builtin:skill_list" | "builtin:skill_load")
			}
			ToolIdentity::Descriptor(reference) => {
				matches!(
					reference.id.as_str(),
					"aidash.skill_list" | "aidash.skill_load"
				)
			}
			_ => false,
		};
		(self.reach == ToolReach::ReceiverMaterial && legacy_skill).then(|| Self {
			reach: ToolReach::Never,
			..self.clone()
		})
	}

	/// The contract a Run pinned under `digest`, if any. A Run admitted before
	/// its provider declared Concurrency safety keeps that operation Sequential;
	/// a legacy Read-only definition keeps its approval-gated Unsafe contract.
	pub fn pinned(mut self, digest: &str) -> crate::Result<Option<Self>> {
		if self.digest()? == digest {
			return Ok(Some(self));
		}
		if let Some(mut previous) = self.legacy_skill_reach() {
			if previous.digest()? == digest {
				return Ok(Some(previous));
			}
			previous.behavior.concurrency = Concurrency::Sequential;
			if previous.digest()? == digest {
				return Ok(Some(previous));
			}
		}
		if self.behavior.fitting == Some(ResultFitting::ThreadRecord)
			&& self.behavior.yields_model_media
		{
			let mut previous = self.clone();
			previous.behavior.yields_model_media = false;
			if previous.digest()? == digest {
				return Ok(Some(previous));
			}
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
			provider_owned: false,
			identity: ToolIdentity::AgentPlugin(identity),
			authorization: ToolAuthorization::none(),
			behavior: ToolBehavior {
				// A read-only tool the Agent version declares still passes the
				// Tool call gate, so an explicit deny refuses it (ADR 0076).
				read_only_operation: read_only,
				..external_behavior(ToolEffect::Unsafe, !read_only)
			},
			disclosure: DisclosureBoundary::External,
			reach: ToolReach::Never,
		}
	}
	/// A tool of an Agent Plugin's stdio server. It runs in the Run's Execution Pod, bounded by
	/// the `mcp_stdio` group's Catalog Approval and the Scratch Space's allowed hosts like the
	/// shell, so it needs no Tool call approval; it stays Unsafe, so an uncertain call is
	/// reconciled and never replayed (ADR 0073). Like a group-bound shell it carries no core
	/// permission: the group's approval decides availability.
	pub fn agent_plugin_stdio(identity: AgentPluginTool) -> Self {
		Self {
			provider_owned: false,
			identity: ToolIdentity::AgentPlugin(identity),
			authorization: ToolAuthorization::none(),
			behavior: external_behavior(ToolEffect::Unsafe, false),
			disclosure: DisclosureBoundary::Local,
			reach: ToolReach::Never,
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
			provider_owned: false,
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
			reach: ToolReach::ExternalServices,
		}
	}
}

/// The behavior of an ordinary tool that no model media, catch-up or yield attributes touch.
fn external_behavior(effect: ToolEffect, tool_call_approval: bool) -> ToolBehavior {
	ToolBehavior {
		connection_request: false,
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
		authority_change: false,
	}
}

/// The Reach of every builtin, listed by name with no default so a new tool must be classified.
/// A Workspace tool a saved Agent pins reaches what its Thread tool does (#1008).
pub fn builtin_reach(name: &str) -> Option<ToolReach> {
	Some(match crate::tool::thread_tools::current(name) {
		"journal_read"
		| "journal_search"
		| "journal_query"
		| "capability_search"
		| "capability_describe"
		| "capability_load"
		| "capability_unload"
		| "task_wait" => ToolReach::Run,
		"task_create" | "task_assign" | "artifact_publish" | "thread_message" | "thread_read"
		| "thread_observe" | "thread_search" | "memory_recall" | "memory_reflect"
		| "memory_mutate" | "file_share" | "attachment_copy" => ToolReach::HomeWorkspace,
		// A remote Run discovers through its Home and hands a Task on through
		// it: the Home serves both commands and grants every onward hop under
		// the whole chain (#654, #1013), so they reach the Home's Thread.
		"agent_discover" | "task_delegate" => ToolReach::HomeWorkspace,
		"skill_list" | "skill_load" | "skill_read" | "skill_asset_read" | "file_read"
		| "file_search" => ToolReach::ReceiverMaterial,
		"subnode_create"
		| "subnode_task"
		| "subnode_add_member"
		| "subnode_remove_member"
		| "subnode_dissolve"
		| "delete_subnode"
		| "subnode_message" => ToolReach::ReceiverAgents,
		"human_request" | "connection_request" => ToolReach::People,
		"outbound_get" => ToolReach::ExternalServices,
		"shell" | "shell_poll" | "shell_cancel" | "code_interpreter" | "python_poll"
		| "python_cancel" | "apply_patch" | "file_save" | "file_remove" | "file_copy"
		| "process_list" | "process_log" | "process_stop" | "live_preview_start"
		| "live_preview_stop" => ToolReach::Execution,
		// An Agent Plugin's stdio MCP server is never remote (ADR 0100), a Duty acts as one
		// person of its own Tenant (ADR 0134), which a remote Run cannot be, and Node tools
		// and node events act as that person too (#1012, #1013).
		"mcp_stdio" | "duty_create" | "duty_update" | "duty_delete" | "duty_list"
		| "duty_notify" | "node_create" | "node_delete" | "setting_request" => ToolReach::Never,
		op if super::configurator::TOOLS.contains(&op) => ToolReach::Never,
		_ => return None,
	})
}

/// The sole builtin assignment table. Grant identity is independent of model alias.
pub fn builtin_contract(name: &str) -> Option<ToolContract> {
	use AuthorizationRequirement::*;
	use ResourceTarget::*;
	let reach = builtin_reach(name)?;
	let mut contract = ToolContract {
		provider_owned: false,
		identity: ToolIdentity::Builtin(format!("builtin:{name}")),
		authorization: ToolAuthorization {
			flag: None,
			core: None,
			requirements: vec![],
		},
		behavior: ToolBehavior {
			connection_request: false,
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
			authority_change: false,
		},
		disclosure: DisclosureBoundary::Home,
		reach,
	};
	let auth = &mut contract.authorization;
	let behavior = &mut contract.behavior;
	let resource = |action, kind, target| Resource {
		action,
		kind,
		target,
	};
	// A Workspace tool a saved Agent pins has its Thread tool's contract.
	let name = crate::tool::thread_tools::current(name);
	match name {
		"journal_read" | "journal_search" | "journal_query" => {
			behavior.effect = ToolEffect::ReadOnly;
			behavior.fitting = match name {
				"journal_read" => Some(ResultFitting::JournalText),
				"journal_search" => Some(ResultFitting::JournalSearch),
				_ => None,
			};
			contract.disclosure = DisclosureBoundary::Local;
		}
		// Reach: ReceiverAgents; each effect rechecks its creator or member identity.
		"subnode_create"
		| "subnode_task"
		| "subnode_add_member"
		| "subnode_remove_member"
		| "delete_subnode"
		| "subnode_dissolve" => {}
		"subnode_message" => {}
		"task_create" => {
			auth.flag = Some(AgentFlag::TaskCreation);
			auth.requirements
				.push(resource("task.create", "thread", Thread));
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
		}
		"thread_message" => {
			auth.requirements
				.push(resource("message.create", "thread", Thread));
		}
		"attachment_copy" => {
			auth.flag = Some(AgentFlag::WorkspaceRetrieval);
			auth.core = Some(CorePermission::Files);
			contract.disclosure = DisclosureBoundary::Local;
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
		"human_request" | "connection_request" | "setting_request" => {
			auth.requirements
				.push(resource("human.request", "run", Run));
			behavior.continuation = Continuation::Human;
			behavior.connection_request = name == "connection_request";
		}
		"agent_discover" => {
			behavior.effect = ToolEffect::ReadOnly;
		}
		"thread_read" | "thread_observe" | "task_wait" | "thread_search" => {
			auth.flag = Some(AgentFlag::WorkspaceRetrieval);
			behavior.effect = ToolEffect::ReadOnly;
			match name {
				"thread_read" => {
					behavior.fitting = Some(ResultFitting::ThreadRecord);
					behavior.catch_up = true;
					behavior.media_pending = true;
					behavior.yields_model_media = true;
				}
				"thread_observe" => {
					behavior.fitting = Some(ResultFitting::Observation);
					behavior.media_pending = true;
				}
				"task_wait" => behavior.continuation = Continuation::Wait,
				_ => {}
			}
		}
		"skill_read" => {
			auth.core = Some(CorePermission::Skills);
			auth.requirements.push(ConfiguredSkill);
			behavior.effect = ToolEffect::ReadOnly;
			behavior.fitting = Some(ResultFitting::SkillText);
			behavior.media_pending = true;
		}
		"skill_asset_read" => {
			auth.core = Some(CorePermission::Skills);
			auth.requirements.push(ConfiguredSkill);
			behavior.effect = ToolEffect::ReadOnly;
			behavior.fitting = Some(ResultFitting::SkillAsset);
			behavior.media_pending = true;
			contract.disclosure = DisclosureBoundary::Local;
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
		}
		"capability_load" | "capability_unload" => {
			behavior.exposure_update = true;
			contract.disclosure = DisclosureBoundary::Local;
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
			if name != "shell_cancel" {
				behavior.continuation = Continuation::Operation;
			}
			contract.disclosure = DisclosureBoundary::Local;
		}
		"code_interpreter" | "python_poll" | "python_cancel" => {
			auth.core = Some(CorePermission::Python);
			if name != "python_cancel" {
				behavior.continuation = Continuation::Operation;
			}
			contract.disclosure = DisclosureBoundary::Local;
		}
		"apply_patch" => {
			auth.core = Some(CorePermission::Patch);
			contract.disclosure = DisclosureBoundary::Local;
		}
		// Save, remove and copy between the Scratch Space and the Working Area (#745).
		"file_save" | "file_remove" | "file_copy" => {
			auth.core = Some(CorePermission::Files);
			contract.disclosure = DisclosureBoundary::Local;
		}
		// A Run's Background processes in its Execution Pod (#745).
		"process_list" | "process_log" | "process_stop" => {
			auth.core = Some(CorePermission::Shell);
			if name != "process_stop" {
				behavior.effect = ToolEffect::ReadOnly;
			}
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
		// The Catalog Approval of the `mcp_stdio` group lets the stdio MCP servers of
		// Agent Plugins run in the Run's Execution Pod (ADR 0073). Their tools are
		// called by their pinned names; this operation does nothing of its own.
		"mcp_stdio" => {
			auth.core = Some(CorePermission::Shell);
			behavior.effect = ToolEffect::Unsafe;
			contract.disclosure = DisclosureBoundary::Local;
		}
		// Duties act as the person at the root of the Run's chain (ADR 0134).
		// A change passes that person's Approval Mode and Approval Rules as a
		// Tool call approval; the use case checks `schedule.create` as them.
		"duty_create" | "duty_update" | "duty_delete" => {
			behavior.effect = ToolEffect::Unsafe;
			behavior.tool_call_approval = true;
		}
		"duty_list" => {
			behavior.effect = ToolEffect::ReadOnly;
		}
		// An Agent creates and deletes Nodes as the person at the root of its
		// Run's chain, who must hold `node.create` (#1012, ADR 0143). A call
		// passes that person's Approval Mode and Approval Rules as a Tool call
		// approval. Creating a Node copies the Tenant's Catalog Approvals and
		// their Agent subjects into the policy, and deleting one fences its
		// admissions, so both are Authority Changes the Run must not block.
		"aidash_api_catalog" | "tenant_overview" => {
			behavior.effect = ToolEffect::ReadOnly;
			behavior.authority_change = true;
		}
		op if super::configurator::TOOLS.contains(&op) => {
			behavior.effect = ToolEffect::Unsafe;
			behavior.tool_call_approval = true;
			behavior.authority_change = true;
		}
		"node_create" | "node_delete" => {
			behavior.effect = ToolEffect::Unsafe;
			behavior.tool_call_approval = true;
			behavior.authority_change = true;
		}
		// An event for another Node's Duty (#1013) carries data out of this
		// Node, so the person at the root of the chain approves it like a
		// change of their Duties. It is input there, never a requester.
		"duty_notify" => {
			behavior.effect = ToolEffect::Unsafe;
			behavior.tool_call_approval = true;
			contract.disclosure = DisclosureBoundary::External;
		}
		_ => return None,
	}
	Some(contract)
}

/// Every builtin `builtin_contract` knows under its current name; `builtin_reach` has no
/// default. The Workspace tools saved Agents pin ([`crate::tool::thread_tools::WORKSPACE_TOOLS`])
/// are known too, with their Thread tool's contract and Reach (#1008).
pub const ALL_BUILTINS: &[&str] = &[
	"journal_read",
	"journal_search",
	"journal_query",
	"capability_search",
	"capability_describe",
	"capability_load",
	"capability_unload",
	"task_wait",
	"task_create",
	"task_assign",
	"artifact_publish",
	"thread_message",
	"thread_read",
	"thread_observe",
	"thread_search",
	"memory_recall",
	"memory_reflect",
	"memory_mutate",
	"file_share",
	"attachment_copy",
	"skill_read",
	"skill_asset_read",
	"file_read",
	"file_search",
	"subnode_create",
	"subnode_task",
	"subnode_add_member",
	"subnode_remove_member",
	"subnode_dissolve",
	"delete_subnode",
	"subnode_message",
	"human_request",
	"connection_request",
	"setting_request",
	"outbound_get",
	"shell",
	"shell_poll",
	"shell_cancel",
	"code_interpreter",
	"python_poll",
	"python_cancel",
	"apply_patch",
	"file_save",
	"file_remove",
	"file_copy",
	"process_list",
	"process_log",
	"process_stop",
	"live_preview_start",
	"live_preview_stop",
	"agent_discover",
	"task_delegate",
	"skill_list",
	"skill_load",
	"mcp_stdio",
	"duty_create",
	"duty_update",
	"duty_delete",
	"duty_list",
	"duty_notify",
	"node_create",
	"node_delete",
	"aidash_api",
	"aidash_api_catalog",
	"tenant_overview",
	"node_update",
	"member_invite",
	"member_set_groups",
	"registry_create",
	"catalog_approve",
	"provider_binding_set",
	"budget_set",
	"approval_mode_set",
	"approval_rule_set",
	"tenant_instructions_set",
	"data_protection_set",
	"tenant_secret_set",
	"stint_start",
	"marketplace_install",
	"mcp_server_add",
	"tool_add",
	"agent_plugin_add",
	"skill_add",
	"reference_upload",
	"duty_template_install",
	"agent_create",
	"generation_policy_set",
	"node_registration",
];

#[cfg(test)]
mod tests {
	use super::ALL_BUILTINS;
	#[test]
	fn command_calls_declare_an_operation_continuation() {
		for name in ["shell", "shell_poll", "code_interpreter", "python_poll"] {
			let contract = super::builtin_contract(name).unwrap();
			assert_eq!(
				serde_json::to_value(contract.behavior.continuation).unwrap(),
				"Operation"
			);
		}
	}
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
			assert_eq!(contract.reach, ToolReach::HomeWorkspace);
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
	fn schedule_changes_need_a_tool_call_approval_and_stay_at_home() {
		for (name, approval, effect) in [
			("duty_create", true, ToolEffect::Unsafe),
			("duty_update", true, ToolEffect::Unsafe),
			("duty_delete", true, ToolEffect::Unsafe),
			("duty_list", false, ToolEffect::ReadOnly),
		] {
			let contract = builtin_contract(name).unwrap();
			assert_eq!(contract.behavior.tool_call_approval, approval, "{name}");
			assert_eq!(contract.behavior.effect, effect, "{name}");
			assert!(!contract.behavior.read_only_operation, "{name}");
			assert_eq!(contract.reach, ToolReach::Never, "{name}");
			assert_eq!(contract.disclosure, DisclosureBoundary::Home);
			assert_eq!(
				crate::tool::providers::core_provider(name),
				Some("core.duties@1")
			);
		}
	}
	#[test]
	fn node_tools_need_a_tool_call_approval_and_release_the_run_lease() {
		for name in ["node_create", "node_delete"] {
			// Act
			let contract = builtin_contract(name).unwrap();
			// Assert
			assert!(contract.behavior.tool_call_approval, "{name}");
			assert!(contract.behavior.authority_change, "{name}");
			assert_eq!(contract.behavior.effect, ToolEffect::Unsafe, "{name}");
			assert_eq!(contract.reach, ToolReach::Never, "{name}");
			assert_eq!(contract.disclosure, DisclosureBoundary::Home);
			assert_eq!(
				crate::tool::providers::core_provider(name),
				Some("core.nodes@1")
			);
			let encoded = serde_json::to_value(&contract).unwrap();
			assert_eq!(encoded["behavior"]["authority_change"], true, "{name}");
		}
		// Every other contract keeps its pinned digest: the flag is omitted.
		let encoded = serde_json::to_value(builtin_contract("duty_create").unwrap()).unwrap();
		assert!(encoded["behavior"].get("authority_change").is_none());
	}

	#[test]
	fn a_node_event_leaves_the_node_only_with_the_persons_approval() {
		let contract = builtin_contract("duty_notify").unwrap();
		assert!(contract.behavior.tool_call_approval);
		assert_eq!(contract.behavior.effect, ToolEffect::Unsafe);
		assert_eq!(contract.disclosure, DisclosureBoundary::External);
		// A Run that arrived through Federation never sends one.
		assert_eq!(contract.reach, ToolReach::Never);
		assert_eq!(
			crate::tool::providers::core_provider("duty_notify"),
			Some("core.duties@1")
		);
	}
	#[test]
	fn every_builtin_has_an_explicit_reach() {
		let by_reach = |reach: ToolReach| {
			let mut names = ALL_BUILTINS
				.iter()
				.copied()
				.filter(|name| builtin_contract(name).unwrap().reach == reach)
				.collect::<Vec<_>>();
			names.sort_unstable();
			names
		};
		let sorted = |mut names: Vec<&'static str>| {
			names.sort_unstable();
			names
		};
		assert_eq!(
			by_reach(ToolReach::Run),
			sorted(vec![
				"journal_read",
				"journal_search",
				"journal_query",
				"capability_search",
				"capability_describe",
				"capability_load",
				"capability_unload",
				"task_wait"
			])
		);
		assert_eq!(
			by_reach(ToolReach::HomeWorkspace),
			sorted(vec![
				"task_create",
				"task_assign",
				"artifact_publish",
				"thread_message",
				"thread_read",
				"thread_observe",
				"thread_search",
				"memory_recall",
				"memory_reflect",
				"memory_mutate",
				"file_share",
				"attachment_copy",
				"agent_discover",
				"task_delegate"
			])
		);
		assert_eq!(
			by_reach(ToolReach::ReceiverMaterial),
			sorted(vec![
				"skill_list",
				"skill_load",
				"skill_read",
				"skill_asset_read",
				"file_read",
				"file_search"
			])
		);
		assert_eq!(
			by_reach(ToolReach::ReceiverAgents),
			sorted(vec![
				"subnode_create",
				"subnode_task",
				"subnode_add_member",
				"subnode_remove_member",
				"subnode_dissolve",
				"delete_subnode",
				"subnode_message"
			])
		);
		assert_eq!(
			by_reach(ToolReach::People),
			sorted(vec!["human_request", "connection_request"])
		);
		assert_eq!(
			by_reach(ToolReach::ExternalServices),
			sorted(vec!["outbound_get"])
		);
		assert_eq!(
			by_reach(ToolReach::Execution),
			sorted(vec![
				"shell",
				"shell_poll",
				"shell_cancel",
				"code_interpreter",
				"python_poll",
				"python_cancel",
				"apply_patch",
				"file_save",
				"file_remove",
				"file_copy",
				"process_list",
				"process_log",
				"process_stop",
				"live_preview_start",
				"live_preview_stop"
			])
		);
		assert_eq!(
			by_reach(ToolReach::Never),
			sorted(
				vec![
					"mcp_stdio",
					"duty_create",
					"duty_update",
					"duty_delete",
					"duty_list",
					"duty_notify",
					"node_create",
					"setting_request",
					"node_delete"
				]
				.into_iter()
				.chain(crate::tool::configurator::TOOLS.iter().copied())
				.collect()
			)
		);
		// A pinned Workspace tool reaches what its Thread tool does (#1008).
		for (current, former) in crate::tool::thread_tools::RENAMED {
			assert!(!ALL_BUILTINS.contains(&former), "{former}");
			assert_eq!(builtin_reach(former), builtin_reach(current), "{former}");
			assert_eq!(
				builtin_contract(former).unwrap().reach,
				builtin_contract(current).unwrap().reach,
				"{former}"
			);
		}
		// A name outside the table has neither a Reach nor a contract.
		assert_eq!(builtin_reach("memory_write"), None);
		assert!(ToolReach::Execution.listable());
		assert!(!ToolReach::Run.listable() && !ToolReach::Never.listable());
	}
	#[test]
	fn modes_are_explicit_sets() {
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
		let thread = uuid::Uuid::new_v4();
		let area = uuid::Uuid::new_v4();
		let input = serde_json::json!({"file_id":file,"expected_digest":"sha256:fixture","representation":"model_input","thread_id":thread,"area_id":area});
		let output = serde_json::json!({"status":"completed","metadata":{"file_id":file}});
		let selected = builtin_contract("file_read")
			.unwrap()
			.behavior
			.model_media_selection(&input, &output)
			.unwrap();
		assert_eq!(
			serde_json::to_value(selected).unwrap(),
			serde_json::json!({"file_id":file,"expected_digest":"sha256:fixture","thread_id":thread,"area_id":area})
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
	fn admitted_builtin_digests_stay_unchanged() {
		// Admitted Runs pin `provider_contract_digest` over these bytes.
		let contract = builtin_contract("workspace_read")
			.unwrap()
			.pinned(PINNED_THREAD_READ_DIGEST)
			.unwrap()
			.unwrap();
		assert_eq!(
			serde_json::to_string(&contract.behavior).unwrap(),
			r#"{"effect":"ReadOnly","fitting":"WorkspaceRecord","continuation":"Ordinary","catch_up":true,"media_pending":true,"yields_model_media":false,"tool_call_approval":false}"#
		);
		assert_eq!(
			crate::registry::rules::digest(&serde_json::to_value(&contract).unwrap()),
			PINNED_THREAD_READ_DIGEST
		);
	}
	#[test]
	fn admitted_thread_reader_descriptor_keeps_its_original_contract() {
		let identity =
			crate::registry::bindings::QualifiedRef::builtin("aidash://node", "workspace_read");
		let descriptor =
			crate::tool::providers::core_descriptor("aidash://node", "workspace_read").unwrap();
		let old_contract = descriptor
			.declared_contract(crate::registry::bindings::QualifiedRef {
				version: "1.0.0".into(),
				..identity.clone()
			})
			.unwrap();
		assert!(!old_contract.behavior.yields_model_media);
		let current = descriptor
			.declared_contract(crate::registry::bindings::QualifiedRef {
				version: "1.1.0".into(),
				..identity
			})
			.unwrap();
		assert!(current.behavior.yields_model_media);
		let mut old = current.clone();
		old.behavior.yields_model_media = false;
		assert_eq!(current.pinned(&old.digest().unwrap()).unwrap(), Some(old));
	}
	#[test]
	fn thread_image_selection_requires_the_exact_returned_file_version() {
		let behavior = builtin_contract("workspace_read").unwrap().behavior;
		let id = uuid::Uuid::from_u128(1);
		let area = uuid::Uuid::from_u128(2);
		let input = serde_json::json!({"kind":"file", "representation":"model_input", "id":id, "area_id":area, "generation":3, "path":"chart.png", "expected_digest":"abc"});
		let mut output = serde_json::json!({"status":"completed", "metadata":{"file_id":id, "path":"chart.png", "digest":"abc"}});
		assert!(matches!(
			behavior.model_media_selection(&input, &output),
			Some(crate::media::ModelMedia::WorkingImage(_))
		));
		assert!(behavior.fitting_for(&input).is_none());
		output["metadata"]["path"] = serde_json::json!("other.png");
		assert!(behavior.model_media_selection(&input, &output).is_none());
	}
	#[test]
	fn thread_attachment_selection_requires_matching_completed_model_input() {
		let behavior = builtin_contract("workspace_read").unwrap().behavior;
		let id = uuid::Uuid::from_u128(1);
		let message_id = uuid::Uuid::from_u128(2);
		let input =
			serde_json::json!({"kind":"attachment", "id":id, "representation":"model_input"});
		let output = serde_json::json!({"status":"completed", "metadata":{"id":id}, "selection":{
			"attachment_id":id, "message_id":message_id, "expected_digest":"sha256:abc", "wav":false
		}});
		assert!(behavior.fitting_for(&input).is_none());
		assert!(matches!(behavior.model_media_selection(&input, &output),
			Some(crate::media::ModelMedia::ThreadAttachment(selection)) if selection.attachment_id == id && selection.message_id == message_id));
		let mut mismatch = output.clone();
		mismatch["selection"]["attachment_id"] = serde_json::json!(uuid::Uuid::from_u128(3));
		assert!(behavior.model_media_selection(&input, &mismatch).is_none());
		let mut metadata_only = input.clone();
		metadata_only["representation"] = serde_json::json!("metadata");
		assert!(
			behavior
				.model_media_selection(&metadata_only, &output)
				.is_none()
		);
		assert!(
			behavior
				.fitting_for(&serde_json::json!({"kind":"message"}))
				.is_some()
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
			assert_eq!(contract.reach, ToolReach::Never);
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
	fn attachment_copy_is_a_write_operation() {
		let contract = builtin_contract("attachment_copy").unwrap();
		assert_eq!(contract.behavior.effect, ToolEffect::Idempotent);
		assert!(!contract.behavior.read_only_operation);
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
				scopes: None,
				secret_owner: Default::default(),
			},
		);
		let plugin = ToolContract::agent_plugin(plugin_tool(), false);

		assert_eq!(plugin.behavior, registry.behavior);
		assert_eq!(plugin.authorization, registry.authorization);
		assert_eq!(plugin.disclosure, registry.disclosure);
		assert_eq!(registry.reach, ToolReach::ExternalServices);
		assert_eq!(plugin.reach, ToolReach::Never);
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
	const PINNED_THREAD_READ_DIGEST: &str =
		"sha256:5ba171e3a421d6ff3f7d254e152b1efbc019e936f77d65e7fc140197b5f66465";
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
			"disclosure": "External", "reach": "external_services"
		});
		let digest = crate::registry::rules::digest(&legacy);
		for config in [
			ToolConfig::Http {
				endpoint: "https://example.invalid".into(),
				replay: "read_only".into(),
				secret: None,
				secret_owner: Default::default(),
			},
			ToolConfig::Mcp {
				endpoint: "https://example.invalid".into(),
				secret: None,
				tool_name: "lookup".into(),
				replay: "read_only".into(),
				idempotency_argument: None,
				scopes: None,
				secret_owner: Default::default(),
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
					secret_owner: Default::default(),
				},
				ToolConfig::Mcp {
					endpoint: "https://example.invalid".into(),
					secret: None,
					tool_name: "lookup".into(),
					replay: replay.into(),
					idempotency_argument: None,
					scopes: None,
					secret_owner: Default::default(),
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
				assert_eq!(contract.reach, ToolReach::ExternalServices);
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
