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
		let Some(authority) = &ctx.home.authority else {
			return Err(Error::Forbidden);
		};
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
/// Provider implementation inventory; capability availability comes from Bindings.
pub(crate) fn implementations() -> BTreeMap<String, Arc<dyn Tool>> {
	let mut tools = BTreeMap::<String, Arc<dyn Tool>>::new();
	use super::{
		approvals::Outbound,
		contracts::*,
		python::Python,
		sharing::Share,
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
			"Read authorized text or metadata. Give file_id for a Working Area or Reference file, or path for a file in the Scratch Space your commands run in (text only, continue with next_offset). Set workspace_id and area_id together to read a Subnode you created. Use model_input with an exact digest to include an image or audio file in the next model inference only.",
			schema::<pod::ToolFileRead>(),
		),
		(
			"file_save",
			"Save files from the Scratch Space into the Working Area so they outlast it and can be shared, attached or read by others. paths are files or directories (\".\" is everything); ignore_files defaults to [\".gitignore\"], exclude beats it and include beats both. Do not save what can be regenerated, such as build output or installed dependencies (node_modules, .venv, target). sync also removes saved files under paths that are gone. A save over a limit is refused whole and names the first file over it.",
			schema::<pod::FileSave>(),
		),
		(
			"file_remove",
			"Remove saved files, or every saved file under a directory, from the Working Area. The Scratch Space is not changed.",
			schema::<pod::FileRemove>(),
		),
		(
			"file_copy",
			"Copy saved Working Area files (deliverables, attachments, references) into the Scratch Space at their paths, or under destination. Files already there are replaced.",
			schema::<pod::FileCopy>(),
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
			"Execute in the live isolated Python kernel and wait for completion without another model call. wait_seconds defaults to 300, is capped at 3600, and zero returns immediately. SESSION_RESET requires acknowledging the new ID; saved files survive heap loss. expected_revision is the revision from the previous result; a new area starts at 1.",
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
			"Apply a Codex add/update/delete patch with the current revision and exact digest-or-absence preconditions for every path. expected_revision is the revision from the previous result; a new area starts at 1. Where commands run in an Execution Pod, the patch applies to the Scratch Space, all or nothing, and the preconditions are checked against the patch's own context instead.",
			schema::<Patch>(),
		),
		(
			"file_share",
			"Share selected immutable files with an exact authorized Agent/thread; delivery cannot recall information already read. expected_revision is the revision from the previous result; a new area starts at 1.",
			schema::<Share>(),
		),
		(
			"live_preview_start",
			"Start this Run's Live preview: a long-lived Web server from the current Working Area revision. 'port' must be the Deployment's preview Pod port (preview.pod_port); any other port is refused. Waits up to 120 s for the port and returns its status and a bounded startup log, never a viewing address. Starting replaces a running preview.",
			schema::<LivePreviewStart>(),
		),
		(
			"live_preview_stop",
			"Stop this Run's Live preview and release the File versions it kept.",
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
