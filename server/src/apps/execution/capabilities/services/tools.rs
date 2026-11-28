use super::CoreCapabilities;
use crate::{
	Error, Result,
	provider::ToolSpec,
	tool::{Tool, ToolContext},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
struct CoreTool {
	name: &'static str,
	contract: aidash_domain::tool::ToolContract,
	schema: Value,
	description: &'static str,
}
#[async_trait::async_trait]
impl Tool for CoreTool {
	fn contract(&self) -> aidash_domain::tool::ToolContract {
		self.contract.clone()
	}
	fn specification(&self) -> ToolSpec {
		ToolSpec {
			name: self.name.into(),
			parameters: self.schema.clone(),
			description: self.description.into(),
		}
	}
	fn replay_safe(&self) -> bool {
		self.contract().replay_safe()
	}
	async fn invoke(&self, ctx: &ToolContext, input: Value, key: &str) -> Result<Value> {
		if self.name == "skill_read" && input.get("skill").is_some() {
			return crate::tool::builtins()
				.get("skill_read")
				.expect("legacy Skill reader")
				.invoke(ctx, input, key)
				.await;
		}
		if self.name == "mcp_stdio" {
			// The group only carries the Tenant's approval (ADR 0073).
			return Err(Error::Invalid(
				"mcp_stdio has no call of its own: call the stdio servers' tools by their names"
					.into(),
			));
		}
		let Some(authority) = ctx.home.tool_authority() else {
			return Err(Error::Forbidden);
		};
		// A Run executing here for another Home reaches the Home's person and the Home's Run
		// through the Home, not through this Node's own people and Areas (ADR 0139). Neither
		// call holds a transaction across its Federation requests.
		if !ctx.run.is_local(ctx.store.deployment.as_str()) {
			match self.name {
				"outbound_get" => {
					return authority
						.outbound_to_home(&ctx.store, &ctx.run, &ctx.home, input, key)
						.await;
				}
				_ => {}
			}
		}
		let shared_read = aidash_domain::tool::concurrency::batchable(&self.contract.behavior)
			&& aidash_domain::tool::concurrency::shares_every_resource(self.name, &input);
		authority
			.core_tool(&ctx.store, &ctx.run, self.name, input, key, shared_read)
			.await
	}
}
/// The model and HTTP APIs share the exact deserialization types. Inline local
/// OpenAPI references because model providers do not resolve component catalogs.
fn schema<T: schemars::JsonSchema>() -> Value {
	let settings = schemars::generate::SchemaSettings::draft07()
		.with(|settings| settings.inline_subschemas = true);
	serde_json::to_value(settings.into_generator().into_root_schema_for::<T>())
		.expect("serializable input schema")
}

pub(crate) fn add(tools: &mut BTreeMap<String, Arc<dyn Tool>>, config: &CoreCapabilities) {
	tools.extend(
		implementations()
			.into_iter()
			.filter(|(name, _)| config.permits(name)),
	);
}
/// The 1.0.0 declarations of `file_read` and `file_search`, before they read the
/// Scratch Space (#745). Databases seeded with them keep these exact bytes.
pub(crate) fn legacy_specifications() -> BTreeMap<String, ToolSpec> {
	use super::contracts::{FileRead, FileSearch};
	BTreeMap::from([
		(
			"file_search".to_owned(),
			ToolSpec {
				name: "file_search".into(),
				description: "Search authorized files with located provenance and revision-bound continuation. In path mode an empty query lists every path in the scope.".into(),
				parameters: schema::<FileSearch>(),
			},
		),
		(
			"file_read".to_owned(),
			ToolSpec {
				name: "file_read".into(),
				description: "Read authorized text or metadata. Set thread_id and area_id together to read a Subnode you created. Use model_input with an exact digest to include an image or audio file in the next model inference only.".into(),
				parameters: schema::<FileRead>(),
			},
		),
	])
}
/// The immutable file tool schemas published with Execution Pods.
pub(crate) fn pre_scratch_specifications() -> BTreeMap<String, ToolSpec> {
	use super::contracts::{Representation, SearchMode};

	/// `file_read` as Agents call it: a Working Area or Reference file by
	/// `file_id`, or a Scratch Space file by `path` (#745).
	#[derive(schemars::JsonSchema)]
	#[serde(deny_unknown_fields)]
	#[allow(dead_code)]
	struct ToolFileRead {
		/// A Working Area or Reference file. Leave it out to read `path`.
		file_id: Option<uuid::Uuid>,
		/// A file in the Scratch Space, relative to its root. Leave out `file_id`.
		path: Option<String>,
		thread_id: Option<uuid::Uuid>,
		area_id: Option<uuid::Uuid>,
		representation: Option<Representation>,
		offset: Option<usize>,
		max_bytes: Option<usize>,
		expected_digest: Option<String>,
	}

	/// Where `file_search` looks; `scratch` is the Run's Scratch Space (#745).
	#[derive(schemars::JsonSchema)]
	#[serde(rename_all = "snake_case")]
	#[allow(dead_code)]
	enum ToolFileScope {
		Working,
		References,
		Received,
		Scratch,
	}

	/// `file_search` as Agents call it (#745).
	#[derive(schemars::JsonSchema)]
	#[serde(deny_unknown_fields)]
	#[allow(dead_code)]
	struct ToolFileSearch {
		/// May be empty in `path` mode, which then lists every path in the scope.
		query: String,
		mode: SearchMode,
		scope: ToolFileScope,
		path: Option<String>,
		/// Scratch Space only: a glob such as `src/*.rs` the whole path must match.
		glob: Option<String>,
		cursor: Option<String>,
		limit: Option<usize>,
	}

	[
		(
			"file_read",
			"Read authorized text or metadata. Give file_id for a Working Area or Reference file, or path for a file in the Scratch Space your commands run in (text only, continue with next_offset). Set thread_id and area_id together to read a Subnode you created. Use model_input with an exact digest to include an image or audio file in the next model inference only.",
			schema::<ToolFileRead>(),
		),
		(
			"file_search",
			"Search authorized files with located provenance and revision-bound continuation. In path mode an empty query lists every path in the scope. scope=scratch searches the Scratch Space your commands run in (literal, regex or path mode, optional glob); its results have no cursor.",
			schema::<ToolFileSearch>(),
		),
	]
	.into_iter()
	.map(|(name, description, parameters)| (name.to_owned(), ToolSpec { name: name.into(), description: description.into(), parameters }))
	.collect()
}
/// Provider implementation inventory; capability availability comes from Bindings.
pub(crate) fn implementations() -> BTreeMap<String, Arc<dyn Tool>> {
	let mut tools = BTreeMap::<String, Arc<dyn Tool>>::new();
	use super::{
		approvals::Outbound,
		contracts::*,
		python::Python,
		skills::{SkillList, SkillLoad, SkillRead},
	};
	use crate::apps::execution::capabilities::serializers::contracts as pod;
	for (name, description, mut parameters) in [
		(
			"file_search",
			"Search authorized files with located provenance and revision-bound continuation. In path mode an empty query lists every path in the scope. scope=scratch searches the Scratch Space your commands run in (literal, regex or path mode, optional glob); its results have no cursor.",
			schema::<pod::ToolFileSearch>(),
		),
		(
			"file_read",
			"Read authorized text or metadata. Give file_id for a Reference file, or path for a file relative to the Scratch Space root. Continue text reads with next_offset. Use model_input to select a Scratch Space image, or use it with the exact digest of a Reference file.",
			schema::<pod::ToolFileRead>(),
		),
		(
			"process_list",
			"List this Run's Background processes in the Execution Pod: what a command left running when it ended (its result names it in left_running) and Live previews.",
			schema::<pod::ProcessList>(),
		),
		(
			"process_log",
			"Read a Background process's stdout or stderr. Without offset it returns the last limit bytes; continue with next_offset.",
			schema::<pod::ProcessLog>(),
		),
		(
			"process_stop",
			"Stop one of this Run's Background processes.",
			schema::<pod::ProcessStop>(),
		),
		(
			"shell",
			"Start a Shell operation and wait for completion without another model call. wait_seconds defaults to 300, is capped at 3600, and zero returns immediately. Cancellation cannot undo earlier effects. expected_revision is the revision from the previous result; a new area starts at 1.",
			schema::<Shell>(),
		),
		(
			"shell_poll",
			"Wait for a Shell operation without polling in a loop. wait_seconds defaults to 300, is capped at 3600, and zero returns immediately. Use next_offset for later output.",
			schema::<OperationInput>(),
		),
		(
			"shell_cancel",
			"Cancel a Shell operation; wait for physical termination confirmation.",
			schema::<OperationInput>(),
		),
		(
			"code_interpreter",
			"Execute in the live isolated Python kernel and wait for completion without another model call. wait_seconds defaults to 300, is capped at 3600, and zero returns immediately. Pass the previous result's session_id as expected_session_id when relying on variables or imports. On SESSION_RESET, rebuild state using the new ID if supplied, or omit expected_session_id to start fresh. Saved files survive heap loss. expected_revision is the revision from the previous result; a new area starts at 1.",
			schema::<Python>(),
		),
		(
			"python_poll",
			"Wait for Python execution without polling in a loop. wait_seconds defaults to 300, is capped at 3600, and zero returns immediately. Use next_offset for later output.",
			schema::<OperationInput>(),
		),
		(
			"python_cancel",
			"Cancel Python execution; earlier filesystem effects may remain.",
			schema::<OperationInput>(),
		),
		(
			"outbound_get",
			"Request a bounded HTTPS GET through the policy broker. Reuse the idempotency key to inspect approval or results; direct sandbox networking remains denied.",
			schema::<Outbound>(),
		),
		(
			"apply_patch",
			"Apply a Codex add/update/delete patch to paths relative to the Scratch Space root.",
			schema::<Patch>(),
		),
		(
			"live_preview_start",
			"Start this Run's Live preview: a long-lived Web server in the Task’s Scratch folder. 'port' must be the Deployment's preview Pod port (preview.pod_port); any other port is refused. Waits up to 120 s for the port and returns its status and a bounded startup log, never a viewing address. Starting replaces a running preview.",
			schema::<LivePreviewStart>(),
		),
		(
			"live_preview_stop",
			"Stop this Run's Live preview.",
			schema::<LivePreviewStop>(),
		),
		(
			"mcp_stdio",
			"Lets the stdio MCP servers of this Agent's plugins run in the Run's Execution Pod. Call their tools by the names they were given; this tool does nothing itself.",
			json!({"type":"object","properties":{},"additionalProperties":false}),
		),
		(
			"skill_list",
			"List pinned Skill metadata. Select by UUID and origin because names can collide.",
			schema::<SkillList>(),
		),
		(
			"skill_load",
			"Load complete immutable Skill instructions and inventory. Loading never executes scripts or raises permissions.",
			schema::<SkillLoad>(),
		),
		(
			"skill_read",
			"Read a bounded pinned Skill file or an exact legacy Registry Skill.",
			schema::<SkillRead>(),
		),
	] {
		if name == "skill_read" {
			let legacy = crate::tool::builtins()
				.get("skill_read")
				.expect("legacy Skill reader")
				.specification()
				.parameters;
			parameters = json!({"oneOf":[parameters,legacy]});
		}
		tools.insert(
			name.into(),
			Arc::new(CoreTool {
				name,
				contract: aidash_domain::tool::builtin_contract(name).expect("declared core tool"),
				description,
				schema: parameters,
			}),
		);
	}
	tools
}

#[cfg(test)]
#[path = "../tests/services_tools_tests.rs"]
mod tests;
