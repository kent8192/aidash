//! Loaded plugin copies for the tests of the modules that consume them.
use super::{AgentPluginRef, LoadedPlugin, MCP_SCHEMA, PLUGIN_SCHEMA, PluginFile, load};
use serde_json::json;

pub(crate) const SOURCE: &str =
	"https://github.com/acme/tools/tree/0123456789abcdef0123456789abcdef01234567";

fn file(path: &str, content: &str) -> PluginFile {
	PluginFile {
		path: path.to_owned(),
		bytes: content.as_bytes().to_vec(),
	}
}

/// A plugin named `name` with one remote server, one skipped server and a Skill per directory.
pub(crate) fn plugin(name: &str, skills: &[&str]) -> LoadedPlugin {
	let mut files = vec![
		file(
			"plugin.json",
			&json!({
				"$schema": PLUGIN_SCHEMA,
				"name": name,
				"version": "1.0.0",
				"license": "MIT",
			})
			.to_string(),
		),
		file(
			"mcp.json",
			&json!({
				"$schema": MCP_SCHEMA,
				"mcpServers": {
					"docs": {"type": "streamable-http", "url": "https://docs.example.com/mcp"},
					"legacy": {"type": "sse", "url": "https://legacy.example.com/sse"},
				},
			})
			.to_string(),
		),
	];
	for skill in skills {
		files.push(file(
			&format!("skills/{skill}/SKILL.md"),
			&format!(
				"---\nname: {skill}\ndescription: Does {skill}.\n---\n\nInstructions for {skill}.\n"
			),
		));
		files.push(file(
			&format!("skills/{skill}/references/guide.md"),
			&format!("Guide for {skill}."),
		));
	}
	load(&files).unwrap_or_else(|rejection| panic!("plugin refused: {rejection:?}"))
}

pub(crate) fn reference(name: &str, skills: &[&str]) -> AgentPluginRef {
	AgentPluginRef::of(&plugin(name, skills), SOURCE)
}
