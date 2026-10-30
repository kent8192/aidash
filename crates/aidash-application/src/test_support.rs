//! Small registered Binding graphs for portable adapter and lifecycle tests.
use crate::{
	Error, Result,
	ports::bindings::{BindingCatalog, ProviderCatalog, StoredAgentPlugin},
};
use aidash_domain::{
	agent_plugin::{AgentPluginRef, MCP_SCHEMA, PLUGIN_SCHEMA, PluginFile},
	registry::{
		Entry, Projection,
		bindings::{BindingSnapshot, QualifiedRef},
	},
	tool::{
		ToolContract,
		providers::{ToolDescriptor, core_descriptor},
	},
};
use async_trait::async_trait;
use futures_util::FutureExt;
use serde_json::json;

/// A compaction Decider configuration as a definition declares it.
pub(crate) fn decider_config() -> serde_json::Value {
	serde_json::to_value(aidash_domain::decision::DeciderConfig {
		hook: aidash_domain::decision::Hook::Compaction,
		answer_type: aidash_domain::decision::AnswerType::Noul,
		description: "Keep needed history".into(),
		provider_credential: "openrouter".into(),
		routes: vec![aidash_domain::decision::DeciderRoute::jev(
			"typesafe/jev-1.14",
		)],
		builder: aidash_domain::decision::BUILDER.into(),
		option_source: aidash_domain::decision::OPTION_SOURCE.into(),
		rule: aidash_domain::decision::RULE.into(),
		mode: aidash_domain::decision::Mode::Enforce,
		max_calls_per_run: 20,
		state_retention: Default::default(),
	})
	.expect("a Decider configuration serializes")
}

pub(crate) fn entry(id: &str, kind: &str, config: serde_json::Value) -> Entry {
	serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"Fixture"},"schema":{"type":"object"},"config":config})).unwrap()
}
pub(crate) fn agent(id: &str) -> Entry {
	entry(
		id,
		"agent",
		json!({"schema_version":1,"model":{"id":"fixture-model","version":"1.0.0"},"instructions":"Do the task.","bindings":[],"remove_default":[]}),
	)
}
pub(crate) const PLUGIN_SOURCE: &str =
	"https://github.com/acme/tools/tree/0123456789abcdef0123456789abcdef01234567";

/// A plugin copy named `name` with a remote server, a skipped server and one Skill per
/// `(directory, body)`.
pub(crate) fn stored_plugin(name: &str, skills: &[(&str, &str)]) -> StoredAgentPlugin {
	stored_plugin_with_servers(
		name,
		json!({
			"docs": {"type": "streamable-http", "url": "https://docs.example.com/mcp"},
			"legacy": {"type": "sse", "url": "https://legacy.example.com/sse"},
		}),
		skills,
	)
}

/// A plugin copy named `name` whose `mcp.json` holds exactly `servers`.
pub(crate) fn stored_plugin_with_servers(
	name: &str,
	servers: serde_json::Value,
	skills: &[(&str, &str)],
) -> StoredAgentPlugin {
	let file = |path: String, content: String| PluginFile {
		path,
		bytes: content.into_bytes(),
	};
	let mut files = vec![
		file(
			"plugin.json".into(),
			json!({"$schema": PLUGIN_SCHEMA, "name": name, "version": "1.0.0"}).to_string(),
		),
		file(
			"mcp.json".into(),
			json!({"$schema": MCP_SCHEMA, "mcpServers": servers}).to_string(),
		),
	];
	for (directory, body) in skills {
		files.push(file(
			format!("skills/{directory}/SKILL.md"),
			format!("---\nname: {directory}\ndescription: Does {directory}.\n---\n\n{body}\n"),
		));
	}
	StoredAgentPlugin {
		source: PLUGIN_SOURCE.into(),
		plugin: aidash_domain::agent_plugin::load(&files).expect("the fixture plugin loads"),
	}
}

pub(crate) fn plugin_reference(stored: &StoredAgentPlugin) -> AgentPluginRef {
	AgentPluginRef::of(&stored.plugin, &stored.source)
}

struct Catalog {
	node: String,
	entries: Vec<Entry>,
	plugins: Vec<StoredAgentPlugin>,
}
#[async_trait]
impl BindingCatalog for Catalog {
	async fn agent_plugin(&mut self, digest: &str) -> Result<Option<StoredAgentPlugin>> {
		Ok(self
			.plugins
			.iter()
			.find(|stored| stored.plugin.digest == digest)
			.cloned())
	}
	async fn definition(&mut self, reference: &QualifiedRef) -> Result<Entry> {
		if reference.registry_deployment != self.node {
			return Err(Error::Forbidden);
		}
		if let Some(entry) = self
			.entries
			.iter()
			.find(|entry| entry.id == reference.id && entry.version == reference.version)
		{
			return Ok(entry.clone());
		}
		if reference.id
			== aidash_domain::registry::bindings::source_defaults::WORKSPACE_OBSERVATION_ID
			&& reference.version
				== aidash_domain::registry::bindings::source_defaults::WORKSPACE_OBSERVATION_VERSION
		{
			return Ok(
				aidash_domain::registry::bindings::source_defaults::workspace_observation()?,
			);
		}
		if reference.id == crate::registry::system::COMPACTION_DECIDER_ID {
			return crate::registry::system::compaction_decider();
		}
		if let Some(operation) = reference.id.strip_prefix("aidash.") {
			return Ok(entry(
				&reference.id,
				"tool",
				serde_json::to_value(core_descriptor(&self.node, operation)?)?,
			));
		}
		Err(Error::NotFound(reference.id.clone()))
	}
	async fn installation(&mut self, _: &Projection) -> Result<()> {
		Err(Error::Forbidden)
	}
	async fn source(&mut self, _: &Entry) -> Result<()> {
		Ok(())
	}
}
struct Providers;
impl ProviderCatalog for Providers {
	fn decision_implementation(
		&self,
		config: &aidash_domain::decision::DeciderConfig,
	) -> Result<String> {
		config.validate()?;
		Ok("fixture-decider".into())
	}
	fn contract(
		&self,
		descriptor: &ToolDescriptor,
		identity: &QualifiedRef,
	) -> Result<ToolContract> {
		Ok(descriptor.declared_contract(identity.clone())?)
	}
	fn implementation(&self, descriptor: &ToolDescriptor) -> Result<String> {
		Ok(format!("{}:portable-test", descriptor.provider))
	}
}
pub(crate) fn resolve(
	node: &str,
	root: &Entry,
	remote: bool,
	extras: Vec<Entry>,
) -> BindingSnapshot {
	resolve_with_plugins(node, root, remote, extras, vec![])
}
pub(crate) fn resolve_with_plugins(
	node: &str,
	root: &Entry,
	remote: bool,
	extras: Vec<Entry>,
	plugins: Vec<StoredAgentPlugin>,
) -> BindingSnapshot {
	let mut entries = extras;
	if !entries.iter().any(|entry| entry.kind == "model") {
		let model = &root.config["model"];
		let mut model_entry = entry(
			model["id"].as_str().unwrap(),
			"model",
			json!({"provider":"openrouter","model_id":"fixture","endpoint":"https://fixture.invalid","context_window":200000,"max_output_tokens":1024,"modalities":["text"],"cost":{}}),
		);
		model_entry.version = model["version"].as_str().unwrap().into();
		entries.push(model_entry);
	}
	crate::registry::bindings::resolve(
		&mut Catalog {
			node: node.into(),
			entries,
			plugins,
		},
		&Providers,
		QualifiedRef {
			registry_deployment: node.into(),
			id: root.id.clone(),
			version: root.version.clone(),
		},
		root,
		remote,
	)
	.now_or_never()
	.expect("fixture catalog never performs I/O")
	.unwrap()
}
pub(crate) fn snapshot(node: &str, id: &str) -> BindingSnapshot {
	resolve(node, &agent(id), true, vec![])
}

/// An explicitly authored integration descriptor; publisher replay labels remain facts-free.
pub(crate) fn http_tool(node: &str, id: &str, alias: &str) -> Entry {
	entry(
		id,
		"tool",
		json!({"registry_deployment":node,"provider":"integration.http@1","operation":"invoke","default_alias":alias,"tier":"integration","transport":{"transport":"http","endpoint":"https://production.example/rpc","replay":"unsafe"}}),
	)
}
pub(crate) fn builtin_entries(node: &str) -> Vec<Entry> {
	let mut entries: Vec<Entry> = crate::registry::system::operations()
		.map(|name| {
			entry(
				&format!("aidash.{name}"),
				"tool",
				json!(core_descriptor(node, name).unwrap()),
			)
		})
		.collect();
	entries
		.push(aidash_domain::registry::bindings::source_defaults::workspace_observation().unwrap());
	entries.push(crate::registry::system::compaction_decider().unwrap());
	entries
}
pub(crate) fn binding(kind: &str, node: &str, id: &str) -> serde_json::Value {
	json!({"kind":kind,"target":{"registry_deployment":node,"id":id,"version":"1.0.0"},"narrow":{}})
}

pub(crate) fn native_memory_entries() -> Vec<Entry> {
	let reference = |id: &str| json!({"id":id,"version":"1.0.0"});
	let zero = json!({"input_per_million":0,"output_per_million":0});
	vec![
		entry(
			"native-embedding",
			"embedding",
			json!({"provider":"openai","endpoint":"https://embedding.invalid","model":"fixture","model_version":"1","dimensions":3}),
		),
		entry("native-reranker", "reranker", json!({"provider":"rrf"})),
		entry(
			"native-tokenizer",
			"tokenizer",
			json!({"provider":"utf8_upper_bound"}),
		),
		entry(
			"native",
			"memory",
			json!({"engine":"hindsight_rust","policy":{
            "extraction":reference("fixture-model"),"derivation":reference("fixture-model"),"reflection":reference("fixture-model"),"embedding":reference("native-embedding"),"reranker":reference("native-reranker"),"tokenizer":reference("native-tokenizer"),
            "prices":{"extraction":zero,"derivation":zero,"reflection":zero,"embedding":zero,"reranker":zero},
            "retention":{"unit_max_age_days":null,"candidate_days":7,"history_days":30,"history_versions":16,"model_result_days":7,"backup_days":7,"purge_after_seconds":60,"purge_batch":32,"max_unit_records":128,"max_model_operations":1024},
            "bounds":{"max_unit_bytes":8192,"max_input_bytes":8192,"max_units":16,"max_candidates":8,"max_entities":8,"max_evidence":8,"max_links":8,"max_graph_hops":3,"max_graph_visits":32,"max_results":4,"max_context_tokens":8192,"max_model_calls":4,"max_model_tokens":8192,"max_cost_micros":10000,"max_retries":2,"max_call_seconds":30},
            "semantic_link_min_similarity_millionths":700000,"learn_from_runs":false,"maintain_observations":false,"refresh_mental_models":false}}),
		),
	]
}
pub(crate) fn native_memory_snapshot(node: &str, id: &str) -> BindingSnapshot {
	let mut root = agent(id);
	root.config["bindings"] = json!([binding("memory", node, "native")]);
	resolve(node, &root, true, native_memory_entries())
}

pub(crate) const PLUGIN_NODE: &str = "aidash://test";

/// A valid snapshot of an Agent taking in `plugins`, with `settings` as its
/// `agent_plugin_settings` (JSON null for none).
pub(crate) fn plugin_snapshot(
	plugins: Vec<StoredAgentPlugin>,
	settings: serde_json::Value,
) -> BindingSnapshot {
	plugin_snapshot_with(plugins, settings, vec![], |_| {})
}

/// [`plugin_snapshot`] over a root Agent that `adjust` may change and Registry entries `extras`.
pub(crate) fn plugin_snapshot_with(
	plugins: Vec<StoredAgentPlugin>,
	settings: serde_json::Value,
	extras: Vec<Entry>,
	adjust: impl FnOnce(&mut Entry),
) -> BindingSnapshot {
	let mut root = agent("agent");
	root.config["agent_plugins"] = json!(plugins.iter().map(plugin_reference).collect::<Vec<_>>());
	if !settings.is_null() {
		root.config["agent_plugin_settings"] = settings;
	}
	adjust(&mut root);
	resolve_with_plugins(PLUGIN_NODE, &root, false, extras, plugins)
}

/// A Run bound to `snapshot`.
pub(crate) fn run_of(snapshot: BindingSnapshot) -> aidash_domain::Run {
	use aidash_domain::{ReadyState, RecoveryState, RunControl, RunState, StateVersion};
	aidash_domain::Run {
		node: "brave-otter-k3m9q2xa".parse().unwrap(),
		id: uuid::Uuid::new_v4(),
		task_id: uuid::Uuid::new_v4(),
		workspace_id: uuid::Uuid::new_v4(),
		home_node: format!(
			"{}/brave-otter-k3m9q2xa",
			snapshot.agent.registry_deployment
		),
		agent_id: snapshot.agent.id.clone(),
		agent_version: snapshot.agent.version.clone(),
		state_version: StateVersion::default(),
		state: RunState::Ready(ReadyState::default()),
		recovery: RecoveryState::default(),
		control: RunControl::Active,
		context: aidash_domain::context::Context {
			binding_snapshot: Some(snapshot.into()),
			..Default::default()
		},
		step: 0,
		revision: 0,
		observed_input_seq: 0,
		ledger_worker_ready: true,
		control_revision: 0,
		control_source: Default::default(),
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: "2026-10-02T00:00:00Z".parse().unwrap(),
	}
}

/// A tool as a server lists it.
pub(crate) fn listed_tool(name: &str) -> aidash_domain::agent_plugin::ListedTool {
	aidash_domain::agent_plugin::ListedTool {
		name: name.into(),
		description: Some(format!("Does {name}.")),
		input_schema: json!({
			"type": "object",
			"properties": {"q": {"type": "string"}},
			"required": ["q"],
			"additionalProperties": false,
		}),
	}
}

/// What a scripted server does when listed.
pub(crate) enum Listed {
	Connection(aidash_domain::agent_plugin::ConnectionCause),
	Tools(Vec<aidash_domain::agent_plugin::ListedTool>),
	Forbidden,
	Failed(crate::ports::tools::ListFailure),
	Invalid,
	/// An error whose text carries what a finding must never show.
	Leaky(String),
	Hang,
}

/// One `tools/call` as the fake sessions saw it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SessionCall {
	pub run: uuid::Uuid,
	pub server: crate::ports::tools::PluginServer,
	pub url: String,
	pub tool: String,
	pub schema_digest: String,
	pub arguments: serde_json::Value,
	pub key: String,
}

/// Sessions with scripted servers. A server nobody scripted refuses its destination.
#[derive(Default)]
pub(crate) struct FakeSessions {
	servers: std::sync::Mutex<
		std::collections::BTreeMap<String, (std::time::Duration, std::sync::Arc<Listed>)>,
	>,
	listings: std::sync::Mutex<Vec<(uuid::Uuid, crate::ports::tools::PluginServer)>>,
	calls: std::sync::Mutex<Vec<SessionCall>>,
	closed: std::sync::Mutex<Vec<uuid::Uuid>>,
	replies: std::sync::Mutex<std::collections::VecDeque<Result<serde_json::Value>>>,
	in_flight: std::sync::atomic::AtomicUsize,
	peak: std::sync::atomic::AtomicUsize,
}
impl FakeSessions {
	pub(crate) fn script(&self, server: &str, listed: Listed) -> &Self {
		self.script_after(server, std::time::Duration::ZERO, listed)
	}
	pub(crate) fn script_after(
		&self,
		server: &str,
		delay: std::time::Duration,
		listed: Listed,
	) -> &Self {
		self.servers
			.lock()
			.unwrap()
			.insert(server.into(), (delay, std::sync::Arc::new(listed)));
		self
	}
	/// The answer of the next `call`; without one, a call answers `{"content":[]}`.
	pub(crate) fn reply(&self, reply: Result<serde_json::Value>) {
		self.replies.lock().unwrap().push_back(reply);
	}
	pub(crate) fn listings(&self) -> Vec<(uuid::Uuid, crate::ports::tools::PluginServer)> {
		self.listings.lock().unwrap().clone()
	}
	pub(crate) fn calls(&self) -> Vec<SessionCall> {
		self.calls.lock().unwrap().clone()
	}
	/// The Runs whose sessions were ended, in order.
	pub(crate) fn closed(&self) -> Vec<uuid::Uuid> {
		self.closed.lock().unwrap().clone()
	}
	/// The most listings that ran at the same time.
	pub(crate) fn peak(&self) -> usize {
		self.peak.load(std::sync::atomic::Ordering::SeqCst)
	}
}
#[async_trait]
impl crate::ports::tools::PluginSessions for FakeSessions {
	async fn list(
		&self,
		run: uuid::Uuid,
		server: &crate::ports::tools::PluginServer,
		_: &aidash_domain::agent_plugin::RemoteServer,
		_auth: Option<&crate::tenant_secrets::CallSecret>,
	) -> Result<Vec<aidash_domain::agent_plugin::ListedTool>> {
		use std::sync::atomic::Ordering::SeqCst;
		self.listings.lock().unwrap().push((run, server.clone()));
		let now = self.in_flight.fetch_add(1, SeqCst) + 1;
		self.peak.fetch_max(now, SeqCst);
		let scripted = self
			.servers
			.lock()
			.unwrap()
			.get(server.server.as_str())
			.cloned();
		let result = match scripted {
			None => Err(Error::Forbidden),
			Some((delay, listed)) => {
				tokio::time::sleep(delay).await;
				match &*listed {
					Listed::Tools(tools) => Ok(tools.clone()),
					Listed::Forbidden => Err(Error::Forbidden),
					Listed::Failed(failure) => Err((*failure).into()),
					Listed::Invalid => Err(Error::Invalid("not a tool list".into())),
					Listed::Connection(cause) => Err(Error::Connection(cause.clone())),
					Listed::Leaky(text) => Err(Error::External(text.clone())),
					Listed::Hang => std::future::pending().await,
				}
			}
		};
		self.in_flight.fetch_sub(1, SeqCst);
		result
	}
	async fn call(
		&self,
		run: uuid::Uuid,
		server: &crate::ports::tools::PluginServer,
		config: &aidash_domain::agent_plugin::RemoteServer,
		tool: &str,
		schema_digest: &str,
		_replay_safe: bool,
		arguments: serde_json::Value,
		key: &str,
		_auth: Option<&crate::tenant_secrets::CallSecret>,
	) -> Result<serde_json::Value> {
		self.calls.lock().unwrap().push(SessionCall {
			run,
			server: server.clone(),
			url: config.url.to_string(),
			tool: tool.into(),
			schema_digest: schema_digest.into(),
			arguments,
			key: key.into(),
		});
		self.replies
			.lock()
			.unwrap()
			.pop_front()
			.unwrap_or_else(|| Ok(json!({"content": []})))
	}
	async fn close(&self, run: uuid::Uuid) {
		self.closed.lock().unwrap().push(run);
	}
}

/// A Node of another Deployment, as its sender's Node Registration pins it.
pub(crate) fn foreign_route(target: &str) -> aidash_domain::federation::route::Route {
	use aidash_domain::{
		federation::route::Route,
		nodes::{
			DeploymentKeyId, NodeAddress,
			registration::{Counterpart, PinnedKey},
		},
	};
	let target = if target
		.strip_prefix("aidash://")
		.unwrap_or(target)
		.contains('/')
	{
		target.to_owned()
	} else {
		format!("{target}/calm-heron-ab12cd34")
	};
	Route {
		tenant: aidash_domain::tenancy::TenantName::new("tenant").unwrap(),
		sender: NodeAddress::parse("aidash://home/quiet-otter-cd34ef56").unwrap(),
		subject: None,
		target: NodeAddress::parse(&target).unwrap(),
		counterpart: Counterpart::OtherDeployment {
			endpoint: "https://foreign.invalid".into(),
			key: PinnedKey {
				key_id: DeploymentKeyId::parse(&"a".repeat(43)).unwrap(),
				public_key_pem: "pem".into(),
			},
		},
		atomic_transactions: true,
	}
}
