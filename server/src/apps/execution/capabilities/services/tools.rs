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
		let Some(authority) = &ctx.home.authority else {
			return Err(Error::Forbidden);
		};
		authority
			.core_tool(&ctx.store, &ctx.run, self.name, input, key)
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
		packages::Install,
		python::Python,
		sharing::Share,
		skills::{SkillList, SkillLoad, SkillRead},
	};
	for (name, description, mut parameters) in [
		(
			"file_search",
			"Search authorized files with located provenance and revision-bound continuation.",
			schema::<FileSearch>(),
		),
		(
			"file_read",
			"Read authorized text or metadata. Use model_input with an exact digest to include an image or audio file in the next model inference only.",
			schema::<FileRead>(),
		),
		(
			"shell",
			"Start an isolated Shell operation. Poll the durable operation ID; cancellation cannot undo earlier effects.",
			schema::<Shell>(),
		),
		(
			"shell_poll",
			"Observe the same Shell operation and bounded output continuation.",
			schema::<OperationInput>(),
		),
		(
			"shell_cancel",
			"Cancel a Shell operation; wait for physical termination confirmation.",
			schema::<OperationInput>(),
		),
		(
			"code_interpreter",
			"Execute in the live isolated Python kernel. SESSION_RESET requires acknowledging the new ID before any code executes; saved files survive heap loss.",
			schema::<Python>(),
		),
		(
			"python_install",
			"Install an explicit set of broker-fetched, hash-verified wheels offline. Each dependency must be supplied or already present in the base image. Replaces the writable package overlay and resets Python memory; no credentials or implicit downloads.",
			schema::<Install>(),
		),
		(
			"python_poll",
			"Observe Python execution or installation and bounded output continuation.",
			schema::<OperationInput>(),
		),
		(
			"python_cancel",
			"Cancel Python execution or installation; earlier filesystem effects may remain.",
			schema::<OperationInput>(),
		),
		(
			"outbound_get",
			"Request a bounded HTTPS GET through the policy broker. Reuse the idempotency key to inspect approval or results; direct sandbox networking remains denied.",
			schema::<Outbound>(),
		),
		(
			"apply_patch",
			"Apply a Codex add/update/delete patch with the current revision and exact digest-or-absence preconditions for every path.",
			schema::<Patch>(),
		),
		(
			"file_share",
			"Share selected immutable files with an exact authorized Agent/thread; delivery cannot recall information already read.",
			schema::<Share>(),
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
