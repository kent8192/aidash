use super::*;
use crate::registry::SkillFile;

fn complete_files() -> Vec<PluginFile> {
	let plugin_json = json!({
		"$schema": PLUGIN_SCHEMA,
		"name": "acme-tools",
		"version": "2.1.0",
		"description": "Tools for Acme",
		"author": {"name": "Ada", "email": "ada@example.com", "url": "https://example.com/ada"},
		"homepage": "https://example.com/acme",
		"repository": "https://github.com/acme/tools",
		"license": "Apache-2.0",
		"keywords": ["acme", "tools"],
		"extensions": {"com.example.client": {"mode": "fast"}},
		"x-internal": true,
	});
	let mcp_json = json!({
		"bin": {"type": "stdio", "command": "./bin/server.js", "args": ["--stdio"], "cwd": "${PLUGIN_DATA}/work"},
		"fetch": {"type": "stdio", "command": "npx", "args": ["-y", "@acme/fetch@1.0.0"], "env": {"LOG": "info"}},
		"docs": {"type": "streamable-http", "url": "https://docs.example.com/mcp", "headers": {"X-Client": "aidash"}},
		"legacy": {"type": "sse", "url": "https://legacy.example.com/sse"},
	});
	vec![
		file("README.md", "# Acme tools"),
		file("plugin.json", &plugin_json.to_string()),
		mcp_file(mcp_json),
		file("bin/server.js", "#!/usr/bin/env node\n"),
		file("skills/lint/SKILL.md", &skill_md("lint", "Lints code.")),
		file("skills/lint/references/rules.md", "# Rules"),
		file(
			"skills/deploy/SKILL.md",
			&skill_md("deploy", "Deploys code."),
		),
		PluginFile {
			path: "skills/deploy/assets/logo.bin".into(),
			bytes: vec![0, 1, 2, 255],
		},
	]
}

#[test]
fn a_complete_plugin_loads_with_exactly_the_expected_parts() {
	let files = complete_files();
	let plugin = loaded(&files);

	assert_eq!(plugin.manifest.name, plugin_name("acme-tools"));
	assert_eq!(plugin.manifest.version.as_deref(), Some("2.1.0"));
	assert_eq!(plugin.manifest.keywords, ["acme", "tools"]);
	assert_eq!(plugin.manifest.license.as_deref(), Some("Apache-2.0"));

	assert_eq!(
		plugin.skills,
		[
			PluginSkill {
				directory: "deploy".into(),
				name: "deploy".into(),
				description: "Deploys code.".into(),
				license: None,
				compatibility: None,
				instructions: skill_md("deploy", "Deploys code."),
				files: vec![SkillFile {
					path: "assets/logo.bin".into(),
					content: "AAEC/w==".into(),
					encoding: Some("base64".into()),
				}],
			},
			PluginSkill {
				directory: "lint".into(),
				name: "lint".into(),
				description: "Lints code.".into(),
				license: None,
				compatibility: None,
				instructions: skill_md("lint", "Lints code."),
				files: vec![SkillFile {
					path: "references/rules.md".into(),
					content: "# Rules".into(),
					encoding: None,
				}],
			},
		]
	);

	assert_eq!(
		plugin.servers,
		BTreeMap::from([
			(
				server_name("bin"),
				McpServer::Stdio(StdioServer {
					command: Command::PluginRelative("bin/server.js".into()),
					args: vec!["--stdio".into()],
					env: BTreeMap::new(),
					cwd: Cwd::Data("work".into()),
				})
			),
			(
				server_name("docs"),
				McpServer::Remote(RemoteServer {
					url: "https://docs.example.com/mcp".parse().unwrap(),
					headers: vec![Header {
						name: "X-Client".into(),
						value: "aidash".into()
					}],
				})
			),
			(
				server_name("fetch"),
				McpServer::Stdio(StdioServer {
					command: Command::Bare(Executable::Npx),
					args: vec!["-y".into(), "@acme/fetch@1.0.0".into()],
					env: BTreeMap::from([("LOG".into(), "info".into())]),
					cwd: Cwd::PluginRoot,
				})
			),
		])
	);

	assert_eq!(
		plugin.findings,
		[
			Finding::ignored(Component::Manifest, Reason::UnknownField, "x-internal"),
			skipped(
				Component::Server("legacy".into()),
				Reason::UnsupportedTransport,
				"sse"
			),
		]
	);
	assert_eq!(plugin.digest, tree_digest(&files));
}

#[test]
fn the_result_does_not_depend_on_the_order_of_the_files() {
	let files = complete_files();
	let mut reversed = files.clone();
	reversed.reverse();
	assert_eq!(loaded(&files), loaded(&reversed));
}

#[test]
fn a_loaded_plugin_survives_storage() {
	let plugin = loaded(&complete_files());
	let stored = serde_json::to_value(&plugin).unwrap();
	assert_eq!(stored["manifest"]["name"], "acme-tools");
	assert_eq!(
		stored["servers"]["docs"]["remote"]["url"],
		"https://docs.example.com/mcp"
	);
	assert_eq!(
		serde_json::from_value::<LoadedPlugin>(stored).unwrap(),
		plugin
	);
}

#[test]
fn stored_names_are_validated_when_read_back() {
	assert!(serde_json::from_value::<PluginName>(json!("Bad Name")).is_err());
	assert!(serde_json::from_value::<ServerName>(json!("bad name")).is_err());
	assert_eq!(
		serde_json::from_value::<ServerName>(json!("ok.name")).unwrap(),
		server_name("ok.name")
	);
}

#[test]
fn the_complete_documents_conform_to_the_vendored_schemas() {
	let files = complete_files();
	let document = |path: &str| -> Value {
		let file = files.iter().find(|file| file.path == path).unwrap();
		serde_json::from_slice(&file.bytes).unwrap()
	};
	let mcp = document("mcp.json");
	let mcp_schema: Value =
		serde_json::from_str(include_str!("../schemas/1.0.0/mcp.schema.json")).unwrap();
	assert!(
		jsonschema::validator_for(&mcp_schema)
			.unwrap()
			.is_valid(&mcp)
	);
}
