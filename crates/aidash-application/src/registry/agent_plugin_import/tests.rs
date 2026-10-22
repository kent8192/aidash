use super::*;
use crate::{Error, ports::registry::FetchedPlugin};
use aidash_domain::agent_plugin::{Component, MCP_SCHEMA, Outcome, PLUGIN_SCHEMA};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Mutex;

struct FakeSource {
	answer: Mutex<Option<Result<PluginFetch>>>,
	requests: Mutex<Vec<PluginImportRequest>>,
}

impl FakeSource {
	fn answering(answer: Result<PluginFetch>) -> Arc<Self> {
		Arc::new(Self {
			answer: Mutex::new(Some(answer)),
			requests: Mutex::new(Vec::new()),
		})
	}
}

#[async_trait]
impl AgentPluginSource for FakeSource {
	async fn fetch(&self, request: PluginImportRequest) -> Result<PluginFetch> {
		self.requests.lock().unwrap().push(request);
		self.answer
			.lock()
			.unwrap()
			.take()
			.expect("one fetch per source")
	}
}

fn request(plugin_path: Option<&str>) -> PluginImportRequest {
	PluginImportRequest {
		url: "https://github.com/acme/plugins".into(),
		plugin_path: plugin_path.map(str::to_owned),
	}
}

fn file(path: &str, content: &str) -> PluginFile {
	PluginFile {
		path: path.into(),
		bytes: content.as_bytes().to_vec(),
	}
}

fn manifest(name: &str) -> PluginFile {
	file(
		"plugin.json",
		&json!({"$schema": PLUGIN_SCHEMA, "name": name}).to_string(),
	)
}

fn fetched(files: Vec<PluginFile>) -> PluginFetch {
	PluginFetch::Fetched(FetchedPlugin {
		root: "plugins/review".into(),
		source: "https://github.com/acme/plugins/tree/0123456789012345678901234567890123456789/plugins/review".into(),
		files,
		denied: vec!["escape".into(), "vendor/lib".into()],
	})
}

#[tokio::test]
async fn a_repository_with_several_plugins_returns_the_choice_untouched() {
	let source = FakeSource::answering(Ok(PluginFetch::Choose(vec![
		String::new(),
		"plugins/review".into(),
	])));
	let result = AgentPluginImport::new(source.clone())
		.import(request(None))
		.await
		.unwrap();
	assert_eq!(
		result,
		PluginImport::Choose(vec![String::new(), "plugins/review".into()])
	);
	assert_eq!(source.requests.lock().unwrap().as_slice(), [request(None)]);
}

#[tokio::test]
async fn a_valid_directory_is_loaded_and_its_provenance_passes_through() {
	let files = vec![
		manifest("review"),
		file(
			"mcp.json",
			&json!({"$schema": MCP_SCHEMA, "mcpServers": {"docs": {"type": "streamable-http", "url": "https://example.com/mcp"}}})
				.to_string(),
		),
		file(
			"skills/triage/SKILL.md",
			"---\nname: triage\ndescription: Sorts issues.\n---\n\nSort them.\n",
		),
	];
	let source = FakeSource::answering(Ok(fetched(files.clone())));
	let result = AgentPluginImport::new(source.clone())
		.import(request(Some("plugins/review")))
		.await
		.unwrap();
	let PluginImport::Loaded {
		source: provenance,
		root,
		denied,
		plugin,
		files: passed,
	} = result
	else {
		panic!("a single plugin is loaded");
	};
	assert_eq!(
		provenance,
		"https://github.com/acme/plugins/tree/0123456789012345678901234567890123456789/plugins/review"
	);
	assert_eq!(root, "plugins/review");
	assert_eq!(denied, ["escape", "vendor/lib"]);
	assert_eq!(passed, files);
	assert_eq!(*plugin, agent_plugin::load(&files).unwrap());
	assert_eq!(plugin.manifest.name.as_str(), "review");
	assert_eq!(plugin.skills.len(), 1);
	assert_eq!(plugin.servers.len(), 1);
	assert_eq!(
		source.requests.lock().unwrap().as_slice(),
		[request(Some("plugins/review"))]
	);
}

#[tokio::test]
async fn a_skipped_part_is_reported_and_does_not_refuse_the_plugin() {
	let files = vec![
		manifest("review"),
		file("skills/broken/SKILL.md", "no front matter"),
	];
	let result = AgentPluginImport::new(FakeSource::answering(Ok(fetched(files))))
		.import(request(None))
		.await
		.unwrap();
	let PluginImport::Loaded { plugin, .. } = result else {
		panic!("the plugin loads");
	};
	assert!(plugin.skills.is_empty());
	assert!(plugin.findings.iter().any(|finding| {
		finding.component == Component::Skill("broken".into())
			&& finding.outcome == Outcome::Skipped
	}));
}

#[tokio::test]
async fn an_invalid_manifest_is_refused_with_the_domain_code() {
	let files = vec![file("plugin.json", "[]")];
	let error = AgentPluginImport::new(FakeSource::answering(Ok(fetched(files))))
		.import(request(None))
		.await
		.unwrap_err();
	assert!(
		matches!(
			&error,
			Error::Domain(aidash_domain::Error::Invalid(message))
				if message.starts_with("AGENT_PLUGIN_INVALID_MANIFEST")
		),
		"{error:?}"
	);
}

#[tokio::test]
async fn a_directory_without_a_manifest_is_refused() {
	let files = vec![file("README.md", "docs")];
	let error = AgentPluginImport::new(FakeSource::answering(Ok(fetched(files))))
		.import(request(None))
		.await
		.unwrap_err();
	assert!(
		matches!(
			&error,
			Error::Domain(aidash_domain::Error::Invalid(message)) if message == "AGENT_PLUGIN_NO_MANIFEST"
		),
		"{error:?}"
	);
}

#[tokio::test]
async fn a_source_failure_reaches_the_caller_unchanged() {
	let source = FakeSource::answering(Err(Error::Invalid("AGENT_PLUGIN_NOT_FOUND".into())));
	let error = AgentPluginImport::new(source)
		.import(request(None))
		.await
		.unwrap_err();
	assert!(matches!(error, Error::Invalid(message) if message == "AGENT_PLUGIN_NOT_FOUND"));
}
