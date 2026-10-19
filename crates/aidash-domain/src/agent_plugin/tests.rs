use super::*;
use serde_json::{Value, json};

mod aliases;
mod complete;
mod expansion;
mod manifest;
mod mcp;
mod reference;
mod schemas;
mod skills;
mod tree;

fn file(path: &str, content: &str) -> PluginFile {
	PluginFile {
		path: path.to_owned(),
		bytes: content.as_bytes().to_vec(),
	}
}

fn manifest_file(name: &str) -> PluginFile {
	file(
		"plugin.json",
		&json!({"$schema": PLUGIN_SCHEMA, "name": name}).to_string(),
	)
}

fn mcp_file(servers: Value) -> PluginFile {
	file(
		"mcp.json",
		&json!({"$schema": MCP_SCHEMA, "mcpServers": servers}).to_string(),
	)
}

fn skill_md(name: &str, description: &str) -> String {
	format!("---\nname: {name}\ndescription: {description}\n---\n\nInstructions for {name}.\n")
}

fn skill(name: &str) -> PluginFile {
	file(
		&format!("skills/{name}/SKILL.md"),
		&skill_md(name, "Does a thing."),
	)
}

fn loaded(files: &[PluginFile]) -> LoadedPlugin {
	load(files).unwrap_or_else(|rejection| panic!("plugin refused: {rejection:?}"))
}

fn plugin_name(name: &str) -> PluginName {
	PluginName::try_from(name.to_owned()).unwrap()
}

fn server_name(name: &str) -> ServerName {
	ServerName::try_from(name.to_owned()).unwrap()
}

fn skipped(component: Component, reason: Reason, detail: &str) -> Finding {
	Finding::skipped(component, reason, detail)
}
