use super::*;
use crate::agent_plugin::{ListedTool, Listing, PluginSettings, fixtures::reference, pin};
use std::collections::BTreeSet;

fn stored() -> Value {
	json!({
		"run_message_summary": "",
		"run_message_summary_seq": 0,
		"media_inferred_seq": 0,
		"history": [],
		"journal": {"head": 0, "imported_through": 0},
		"usage": null,
		"compactions": 0,
		"message_read_coverage": {},
		"message_inference_coverage": {}
	})
}

fn pinned() -> crate::agent_plugin::PluginTools {
	let plugin = reference("acme-tools", &[]);
	let tool = |name: &str| ListedTool {
		name: name.into(),
		description: Some(format!("Does {name}.")),
		input_schema: json!({"type": "object"}),
	};
	let listed = [(
		(plugin.name.clone(), "docs".to_owned().try_into().unwrap()),
		Listing::Listed(vec![tool("search"), tool("")]),
	)]
	.into();
	pin(&[plugin], &PluginSettings::new(), &listed, &BTreeSet::new())
}

#[test]
fn a_context_without_plugin_tools_keeps_its_bytes() {
	let stored = stored();

	let context: Context = serde_json::from_value(stored.clone()).unwrap();
	let reloaded = serde_json::to_value(&context).unwrap();

	assert!(context.plugin_tools.is_none());
	assert!(reloaded.get("plugin_tools").is_none());
	assert!(
		serde_json::to_value(context.inspection())
			.unwrap()
			.get("plugin_tools")
			.is_none()
	);
	assert_eq!(
		serde_json::from_value::<Context>(reloaded.clone())
			.map(|context| serde_json::to_value(&context).unwrap())
			.unwrap(),
		reloaded
	);
}

#[test]
fn the_plugin_tools_a_run_pinned_survive_storage() {
	let mut context: Context = serde_json::from_value(stored()).unwrap();
	let pinned = pinned();
	assert_eq!(pinned.servers[0].tools.len(), 1);
	assert_eq!(pinned.findings.len(), 1);
	context.plugin_tools = Some(pinned.clone());

	let encoded = serde_json::to_value(&context).unwrap();
	let reloaded: Context = serde_json::from_value(encoded.clone()).unwrap();

	assert_eq!(
		encoded["plugin_tools"]["servers"][0]["tools"][0]["alias"],
		"acme-tools__docs__search"
	);
	assert_eq!(reloaded.plugin_tools, Some(pinned));
	assert_eq!(serde_json::to_value(&reloaded).unwrap(), encoded);
}

#[test]
fn an_inspection_shows_what_the_run_pinned_and_what_it_left_out() {
	let mut context: Context = serde_json::from_value(stored()).unwrap();
	context.plugin_tools = Some(pinned());

	let inspected = serde_json::to_value(context.inspection()).unwrap();

	assert_eq!(
		inspected["plugin_tools"],
		serde_json::to_value(&context.plugin_tools).unwrap()
	);
	assert_eq!(
		inspected["plugin_tools"]["findings"][0]["finding"]["reason"],
		"tool_invalid"
	);
}

#[test]
fn stored_plugin_tools_refuse_unknown_fields() {
	let mut encoded = stored();
	encoded["plugin_tools"] = json!({"servers": [], "findings": [], "extra": true});

	assert!(serde_json::from_value::<Context>(encoded).is_err());
}
