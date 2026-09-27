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
	schema: Value,
	description: &'static str,
}
#[async_trait::async_trait]
impl Tool for CoreTool {
	fn specification(&self) -> ToolSpec {
		ToolSpec {
			name: self.name.into(),
			parameters: self.schema.clone(),
			description: self.description.into(),
		}
	}
	fn replay_safe(&self) -> bool {
		true
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
fn schema<T: utoipa::ToSchema>() -> Value {
	let mut children = vec![];
	T::schemas(&mut children);
	let mut catalog = children
		.into_iter()
		.map(|(name, schema)| {
			(
				name,
				serde_json::to_value(schema).expect("serializable schema"),
			)
		})
		.collect::<BTreeMap<_, _>>();
	let root = serde_json::to_value(T::schema()).expect("serializable schema");
	catalog.insert(T::name().into_owned(), root.clone());
	fn inline(value: Value, catalog: &BTreeMap<String, Value>, depth: usize) -> Value {
		assert!(depth < 32, "recursive core input schema");
		if let Some(reference) = value.get("$ref").and_then(Value::as_str) {
			let name = reference
				.strip_prefix("#/components/schemas/")
				.expect("local schema");
			return inline(
				catalog.get(name).expect("registered core schema").clone(),
				catalog,
				depth + 1,
			);
		}
		match value {
			Value::Object(fields) => Value::Object(
				fields
					.into_iter()
					.map(|(name, value)| (name, inline(value, catalog, depth + 1)))
					.collect(),
			),
			Value::Array(values) => Value::Array(
				values
					.into_iter()
					.map(|value| inline(value, catalog, depth + 1))
					.collect(),
			),
			scalar => scalar,
		}
	}
	inline(root, &catalog, 0)
}
pub(crate) fn add(tools: &mut BTreeMap<String, Arc<dyn Tool>>, config: &CoreCapabilities) {
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
			"Read an authorized file by ID with UTF-8 byte continuation; binary files support metadata.",
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
		if !config.permits(name) {
			continue;
		}
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
				description,
				schema: parameters,
			}),
		);
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn output_envelope_schema_accepts_the_serialized_wire_contract() {
		use crate::capabilities::contracts::*;
		let envelope = Envelope {
			operation_id: "read-only-call".into(),
			status: "completed".into(),
			policy_revision: 1,
			area_id: uuid::Uuid::nil(),
			generation: 1,
			revision: 1,
			result: CapabilityResult::Search(SearchResult {
				matches: vec![],
				unavailable: vec![SearchUnavailable {
					file_id: uuid::Uuid::nil(),
					error: "REPRESENTATION_UNAVAILABLE".into(),
				}],
				next_cursor: None,
				truncated: false,
			}),
		};
		let wire = serde_json::to_value(&envelope).unwrap();
		let schema = schema::<Envelope>();
		jsonschema::validator_for(&schema)
			.unwrap()
			.validate(&wire)
			.unwrap_or_else(|e| panic!("{e}: {schema}"));
		assert!(serde_json::from_value::<Envelope>(wire).is_ok());
	}
	#[test]
	fn executable_contracts_are_derived_and_disabled_by_default() {
		let mut tools = BTreeMap::new();
		add(&mut tools, &CoreCapabilities::default());
		assert!(tools.is_empty());
		add(
			&mut tools,
			&CoreCapabilities {
				files: true,
				shell: true,
				python: true,
				patch: true,
				skills: true,
				sharing: true,
			},
		);
		assert_eq!(tools.len(), 15);
		for (name, tool) in &tools {
			let schema = tool.specification().parameters;
			assert!(!schema.to_string().contains("$ref"), "{name}");
			if name != "skill_read" {
				assert_eq!(schema["additionalProperties"], false, "{name}");
			}
		}
		let install = tools["python_install"].specification().parameters;
		assert_eq!(
			install["properties"]["wheels"]["items"]["additionalProperties"],
			false
		);
		assert!(
			install["required"]
				.as_array()
				.unwrap()
				.contains(&json!("wheels"))
		);
	}
}
