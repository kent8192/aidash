use super::*;
use crate::ports::{Credentials, registry::*};
use aidash_domain::{
	model::ModelConfig,
	registry::{EntityRef, Projection},
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::{
	collections::{BTreeMap, BTreeSet},
	sync::{Arc, Mutex},
};

#[derive(Default)]
struct Secrets(Mutex<Vec<String>>);
impl Credentials for Secrets {
	fn resolve(&self, reference: &str) -> Result<String> {
		self.0.lock().unwrap().push(reference.into());
		Err(Error::Invalid(format!(
			"credential reference {reference} is not configured"
		)))
	}
}
struct CoreContracts;
impl CoreToolCatalog for CoreContracts {
	fn specifications(
		&self,
		_: &aidash_domain::capabilities::CoreCapabilities,
	) -> BTreeMap<String, aidash_domain::provider::ToolSpec> {
		BTreeMap::new()
	}
}
#[fixture]
fn validation() -> DefinitionValidation {
	DefinitionValidation::new(Arc::new(Secrets::default()), Arc::new(CoreContracts))
}
fn validate(entry: &Entry) -> Result<()> {
	validation().validate_in(entry, true)
}
fn skill_instructions(entry: &Entry) -> Result<String> {
	aidash_domain::registry::rules::skill_instructions(entry).map_err(Into::into)
}

#[derive(Default)]
struct Scope {
	entries: BTreeMap<String, Entry>,
	overlays: BTreeMap<String, Value>,
	pages: Vec<(String, String, Value)>,
	generated: BTreeSet<String>,
	trace: Vec<String>,
	events: Vec<(String, Value)>,
	documents: Option<Value>,
	fail_documents: bool,
}

#[async_trait]
impl PrivateKnowledgeRead for Scope {
	async fn documents(&mut self, _: &Entry) -> Result<Option<Value>> {
		self.trace.push("private_read".into());
		Ok(self.documents.clone())
	}
}
#[async_trait]
impl PrivateKnowledgeScope for Scope {
	async fn insert_documents(&mut self, _: &Entry, documents: Value) -> Result<()> {
		self.trace.push("private_documents".into());
		if self.fail_documents {
			return Err(Error::Conflict("private documents changed".into()));
		}
		self.documents = Some(documents);
		Ok(())
	}
}
fn identity(id: &str, version: &str) -> String {
	format!("{id}@{version}")
}
impl Scope {
	fn put(&mut self, entry: Entry) {
		self.entries
			.insert(identity(&entry.id, &entry.version), entry);
	}
	fn event(&mut self, kind: &str, payload: Value) {
		self.trace.push(kind.into());
		self.events.push((kind.into(), payload));
	}
}
#[async_trait]
impl DefinitionLookup for Scope {
	async fn definition(&mut self, id: &str, version: &str) -> Result<Entry> {
		self.trace.push(format!("read:{id}@{version}"));
		self.entries
			.get(&identity(id, version))
			.cloned()
			.ok_or_else(|| Error::NotFound(id.into()))
	}
	async fn overrides(&mut self, id: &str, version: &str) -> Result<Option<Value>> {
		self.trace.push(format!("overlay:{id}@{version}"));
		Ok(self.overlays.get(&identity(id, version)).cloned())
	}
}
#[async_trait]
impl DefinitionWriter for Scope {
	async fn insert_definition(&mut self, entry: &Entry) -> Result<bool> {
		self.trace.push("insert".into());
		if let Some(existing) = self.entries.get(&identity(&entry.id, &entry.version)) {
			if existing != entry {
				return Err(Error::Conflict("immutable definition".into()));
			}
			return Ok(false);
		}
		self.put(entry.clone());
		Ok(true)
	}
}
#[async_trait]
impl RegistrationScope for Scope {
	async fn assign_id(&mut self, entry: &mut Entry, _: Option<Uuid>) -> Result<()> {
		self.trace.push("assign".into());
		if entry.id.is_empty() {
			entry.id = "assigned".into();
		}
		Ok(())
	}
	async fn append_event(&mut self, kind: &str, payload: Value) -> Result<()> {
		self.event(kind, payload);
		Ok(())
	}
}
#[async_trait]
impl RegistryRead for Scope {
	async fn definitions(
		&mut self,
		_: Option<&str>,
		offset: usize,
		limit: Option<usize>,
	) -> Result<Vec<DefinitionDocument>> {
		Ok(self
			.pages
			.iter()
			.skip(offset)
			.take(limit.unwrap_or(usize::MAX))
			.map(|(id, version, metadata)| DefinitionDocument {
				id: id.clone(),
				version: version.clone(),
				metadata: metadata.clone(),
			})
			.collect())
	}
	async fn generated(&mut self, id: &str, version: &str) -> Result<bool> {
		Ok(self.generated.contains(&identity(id, version)))
	}
}
#[async_trait]
impl PackageScope for Scope {
	async fn publish(
		&mut self,
		id: &str,
		version: &str,
		manifest: Value,
		digest: &str,
		_: &str,
	) -> Result<(PackageRecord, bool)> {
		self.trace.push("publish".into());
		Ok((
			PackageRecord {
				id: id.into(),
				version: version.into(),
				manifest,
				digest: digest.into(),
			},
			true,
		))
	}
	async fn install(&mut self, _: &Entry, _: &str, _: Value) -> Result<bool> {
		self.trace.push("install".into());
		Ok(true)
	}
	async fn append_event(&mut self, kind: &str, payload: Value) -> Result<()> {
		self.event(kind, payload);
		Ok(())
	}
}
fn definition(id: &str, kind: &str, config: Value) -> Entry {
	let name = if id.is_empty() { "Fixture" } else { id };
	serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":name},"description":{"en":"fixture"},"config":config})).unwrap()
}
fn package() -> Package {
	Package {
		entity: definition(
			"skill",
			"skill",
			json!({"instructions":"Use the original evidence"}),
		),
		author: "author".into(),
		permissions: Vec::new(),
		dependencies: Vec::new(),
	}
}
fn snapshot(package: &Package) -> PackageSnapshot {
	let manifest = json!(package);
	PackageSnapshot {
		source: manifest.to_string(),
		digest: digest(&manifest),
		manifest,
	}
}

#[rstest]
#[case::legacy(false, "overlaid")]
#[case::installed(true, "original")]
#[tokio::test]
async fn installed_run_never_resolves_mutable_overlays(
	#[case] installed: bool,
	#[case] expected: &str,
) {
	let mut scope = Scope::default();
	let mut root = definition("agent", "agent", json!({}));
	root.installation = installed.then(|| Projection {
		contract: 1,
		tenant: "tenant".into(),
		installation: "installation".into(),
		revision: 3,
	});
	scope.put(root);
	scope.put(definition(
		"skill",
		"skill",
		json!({"instructions":"original"}),
	));
	scope
		.overlays
		.insert("skill@1.0.0".into(), json!({"instructions":"overlaid"}));
	let run = aidash_domain::RunMetadata {
		id: Uuid::new_v4(),
		task_id: Uuid::new_v4(),
		workspace_id: Uuid::new_v4(),
		home_node: "aidash://home".into(),
		agent_id: "agent".into(),
		agent_version: "1.0.0".into(),
		phase: aidash_domain::RunPhase::Ready,
		control: aidash_domain::RunControl::Active,
		step: 0,
		revision: 0,
		observed_input_seq: 0,
		ledger_worker_ready: true,
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: chrono::Utc::now(),
	};
	let entry = get_for_run(&mut scope, run, "skill", "1.0.0")
		.await
		.unwrap();
	assert_eq!(entry.config["instructions"], expected);
	assert_eq!(
		scope
			.trace
			.iter()
			.any(|event| event.starts_with("overlay:")),
		!installed
	);
}

#[rstest]
#[tokio::test]
async fn registration_rejects_wrong_reference_kind_before_any_write(
	validation: DefinitionValidation,
) {
	let mut scope = Scope::default();
	scope.put(definition(
		"model",
		"skill",
		json!({"instructions":"not a model"}),
	));
	let agent = definition(
		"agent",
		"agent",
		json!({"model":{"id":"model","version":"1.0.0"},"instructions":"execute"}),
	);
	let error = register_definition(&mut scope, &validation, &agent, "aidash://home")
		.await
		.unwrap_err();
	assert_eq!(error.to_string(), "model must reference a model");
	assert_eq!(scope.trace, ["read:model@1.0.0", "overlay:model@1.0.0"]);
	assert!(scope.events.is_empty());
}

#[rstest]
#[tokio::test]
async fn idempotent_registration_emits_one_event_and_preserves_input(
	validation: DefinitionValidation,
) {
	let mut scope = Scope::default();
	let mut input = package().entity;
	input.id.clear();
	let first = register(
		&mut scope,
		&validation,
		input.clone(),
		None,
		true,
		"aidash://home",
	)
	.await
	.unwrap();
	let replay = register(&mut scope, &validation, input, None, true, "aidash://home")
		.await
		.unwrap();
	assert_eq!(first, replay);
	assert_eq!(
		scope.trace,
		[
			"assign",
			"insert",
			"registry.registered",
			"assign",
			"insert"
		]
	);
	assert_eq!(
		scope.events,
		[(
			"registry.registered".into(),
			json!({"id":"assigned","version":"1.0.0","kind":"skill"})
		)]
	);
}

#[rstest]
#[tokio::test]
async fn discovery_exclusions_are_counted_before_decoding_metadata() {
	let mut scope = Scope::default();
	scope.pages.push((
		"installed".into(),
		"1.0.0".into(),
		json!({"installation":null}),
	));
	scope.pages.push((
		"generated".into(),
		"1.0.0".into(),
		json!({"invalid":"excluded metadata"}),
	));
	scope.generated.insert("generated@1.0.0".into());
	for index in 0..64 {
		let entry = definition(&format!("agent-{index:03}"), "agent", json!({}));
		scope
			.pages
			.push((entry.id.clone(), entry.version.clone(), json!(entry)));
	}
	let page = legacy_agents(&mut scope, &Search::default(), 0)
		.await
		.unwrap();
	assert_eq!(page.entries.len(), 64);
	assert_eq!(page.entries[0].id, "agent-000");
	assert_eq!(page.entries[63].id, "agent-063");
	assert_eq!(page.next_offset, Some(66));
	let empty = legacy_agents(&mut scope, &Search::default(), 66)
		.await
		.unwrap();
	assert!(empty.entries.is_empty());
	assert_eq!(empty.next_offset, None);
}

#[rstest]
#[case::raw_bytes(true, false)]
#[case::decoded_value(false, true)]
fn installation_checks_source_bytes_and_decoded_manifest(
	#[case] bytes: bool,
	#[case] manifest: bool,
) {
	let mut snapshot = snapshot(&package());
	let expected = snapshot.digest.clone();
	if bytes {
		snapshot.source.push(' ');
	}
	if manifest {
		snapshot.manifest["author"] = json!("changed");
	}
	assert_eq!(
		prepare_install(snapshot, &expected, json!({}))
			.err()
			.unwrap()
			.to_string(),
		"package digest changed"
	);
}

#[rstest]
#[tokio::test]
async fn missing_package_dependency_prevents_install_and_event(validation: DefinitionValidation) {
	let mut package = package();
	package.dependencies.push(EntityRef {
		id: "missing".into(),
		version: "1.0.0".into(),
	});
	let snapshot = snapshot(&package);
	let digest = snapshot.digest.clone();
	let plan = prepare_install(snapshot, &digest, json!({})).unwrap();
	let mut scope = Scope::default();
	let error = install(
		&mut scope,
		&validation,
		plan,
		"aidash://home",
		"skill",
		"1.0.0",
	)
	.await
	.unwrap_err();
	assert_eq!(error.to_string(), "missing");
	assert_eq!(scope.trace, ["read:missing@1.0.0"]);
	assert!(scope.events.is_empty());
}

#[rstest]
#[case::remote(false, 0)]
#[case::local(true, 1)]
fn remote_structure_validation_does_not_resolve_local_credentials(
	#[case] local: bool,
	#[case] resolutions: usize,
) {
	let secrets = Arc::new(Secrets::default());
	let validation = DefinitionValidation::new(secrets.clone(), Arc::new(CoreContracts));
	let embedding = definition(
		"embedding",
		"embedding",
		json!({"provider":"openai","endpoint":"https://embedding.example.test","credential_env":"AIDASH_SECRET_EMBEDDING","model":"text","model_version":"1","dimensions":3}),
	);
	let result = validation.validate_in(&embedding, local);
	assert_eq!(result.is_ok(), !local);
	assert_eq!(secrets.0.lock().unwrap().len(), resolutions);
}

#[rstest]
#[case::missing(
	0,
	"private documents are unavailable on this node; run the original agent on its owning node"
)]
#[case::changed(1, "private document digest mismatch")]
#[case::original(2, "")]
#[tokio::test]
async fn private_context_requires_the_original_node_content(
	#[case] state: u8,
	#[case] error: &str,
) {
	let documents =
		json!([{"name":"private.pdf","media_type":"application/pdf","text":"private evidence"}]);
	let entry = definition(
		"agent",
		"agent",
		json!({"model":{"id":"model","version":"1.0.0"},"knowledge_digest":aidash_domain::registry::knowledge::digest(&documents)}),
	);
	let mut scope = Scope {
		documents: match state {
			0 => None,
			1 => Some(json!([])),
			_ => Some(documents.clone()),
		},
		..Default::default()
	};
	let result = personal::load(&mut scope, &entry).await;
	if error.is_empty() {
		assert_eq!(result.unwrap(), documents);
	} else {
		assert_eq!(result.unwrap_err().to_string(), error);
	}
	assert_eq!(scope.trace, ["private_read"]);
}

#[rstest]
#[case::accepted(false)]
#[case::changed_private_content(true)]
#[tokio::test]
async fn private_registration_emits_only_after_private_content_is_saved(
	validation: DefinitionValidation,
	#[case] fail_documents: bool,
) {
	let mut scope = Scope {
		fail_documents,
		..Default::default()
	};
	scope.put(definition("model", "model", json!({"provider":"openrouter","model_id":"fixture","endpoint":"https://example.test","context_window":131072,"max_output_tokens":1024,"modalities":["text"],"cost":{}})));
	let entry = definition(
		"",
		"agent",
		json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Use the private evidence"}),
	);
	let draft = personal::validate_input(
		entry,
		vec![aidash_domain::registry::ReferenceDocument {
			name: "private.pdf".into(),
			media_type: "application/pdf".into(),
			text: "PRIVATE-TEXT".into(),
		}],
	)
	.unwrap();
	let prepared = personal::prepare(&mut scope, &validation, draft)
		.await
		.unwrap();
	assert!(!json!(prepared.entry).to_string().contains("PRIVATE-TEXT"));
	scope.trace.clear();
	let result = personal::register(
		&mut scope,
		&validation,
		prepared,
		Uuid::new_v4(),
		"aidash://home",
	)
	.await;
	assert_eq!(result.is_ok(), !fail_documents);
	if fail_documents {
		assert_eq!(result.unwrap_err().to_string(), "private documents changed");
		assert_eq!(scope.events.len(), 0);
		assert_eq!(scope.trace.last().unwrap(), "private_documents");
	} else {
		let entry = result.unwrap();
		assert_eq!(
			scope.trace[scope.trace.len() - 3..],
			["insert", "private_documents", "registry.registered"]
		);
		assert_eq!(
			scope.events,
			[(
				"registry.registered".into(),
				json!({"id":"assigned","version":"1.0.0","kind":"agent"})
			)]
		);
		assert_eq!(
			entry.config["knowledge_digest"],
			aidash_domain::registry::knowledge::digest(scope.documents.as_ref().unwrap())
		);
	}
}

#[test]
fn run_media_routes_follow_current_evidence_and_model_modalities() {
	let now = chrono::Utc::now();
	let mut model: ModelConfig = serde_json::from_value(json!({
		"provider":"openrouter", "model_id":"fixture", "endpoint":"https://example.test",
		"credential_env":null, "context_window":8192, "modalities":["text","image","audio"],
		"cost":{}, "media_routes":[
			{"tag":"image", "formats":["image/png"], "source":"test", "verified_at":now - chrono::Duration::hours(1), "expires_at":now + chrono::Duration::hours(1)},
			{"tag":"audio", "formats":["wav"], "source":"test", "verified_at":now - chrono::Duration::hours(1), "expires_at":now + chrono::Duration::hours(1)},
			{"tag":"expired", "formats":["image/jpeg"], "source":"test", "verified_at":now - chrono::Duration::hours(2), "expires_at":now - chrono::Duration::hours(1)}
		]
	})).unwrap();
	assert_eq!(
		model.current_media_input_routes(),
		vec![vec!["image/png"], vec!["audio/wav", "audio/x-wav"],]
	);
	model.modalities = vec!["text".into()];
	assert!(model.current_media_input_routes().is_empty());
}

fn entry() -> Entry {
	serde_json::from_value(json!({"id":"research","version":"1.0.0","kind":"skill","name":{"en":"Research","ja":"調査"},"description":{"en":"Research"},"capabilities":["web.search"],"languages":["ja","en"],"config":{"instructions":"Research carefully"}})).unwrap()
}
#[rstest::fixture]
fn behavior_config(
	#[default(None)] creation: Option<bool>,
	#[default(None)] delegation: Option<bool>,
) -> AgentConfig {
	serde_json::from_value(json!({"model":{"id":"model","version":"1.0.0"},"allow_task_creation":creation,"allow_task_delegation":delegation})).unwrap()
}
#[rstest::rstest]
#[case(None, None, true, true)]
#[case(Some(true), Some(false), true, false)]
#[case(Some(false), Some(true), false, true)]
#[case(Some(false), Some(false), false, false)]
fn task_assignment_obeys_delegation_independently_of_creation(
	#[case] _creation: Option<bool>,
	#[case] _delegation: Option<bool>,
	#[case] creates: bool,
	#[case] delegates: bool,
	#[with(_creation, _delegation)] behavior_config: AgentConfig,
) {
	assert_eq!(behavior_config.permits_builtin("task_create"), creates);
	assert_eq!(behavior_config.permits_builtin("task_delegate"), delegates);
	assert_eq!(behavior_config.permits_builtin("task_assign"), delegates);
}
#[rstest::rstest]
fn conjunctive_search_and_localization() {
	let e = entry();
	validate(&e).unwrap();
	assert!(
		Search {
			capability: Some("web.search".into()),
			language: Some("JA".into()),
			..Default::default()
		}
		.matches(&e)
	);
	assert!(
		!Search {
			capability: Some("web.search".into()),
			language: Some("fr".into()),
			..Default::default()
		}
		.matches(&e)
	);
	assert!(
		Search {
			query: Some("調査".into()),
			..Default::default()
		}
		.matches(&e)
	);
}
#[rstest::rstest]
fn bundled_skill_files_have_bounded_safe_paths_and_are_listed_on_demand() {
	let mut e = entry();
	e.config = json!({"instructions":"Read the guide when relevant","files":[{"path":"references/guide.md","content":"Evidence"}]});
	validate(&e).unwrap();
	assert!(
		skill_instructions(&e)
			.unwrap()
			.contains("references/guide.md")
	);
	assert!(!skill_instructions(&e).unwrap().contains("Evidence"));
	e.config["files"] = json!([{"path":"assets/logo.png","content":"AAEC","encoding":"base64"}]);
	validate(&e).unwrap();
	assert!(
		skill_instructions(&e)
			.unwrap()
			.contains("binary, base64 encoded")
	);
	e.config["files"][0]["content"] = json!("not base64");
	assert!(validate(&e).is_err());
	e.config["files"][0]["content"] = json!("AAEC");
	e.config["files"][0]["encoding"] = json!("unknown");
	assert!(validate(&e).is_err());
	e.config["files"] = json!([{"path":"references/guide.md","content":"Evidence"}]);
	for path in [
		"../secret",
		"/absolute",
		"references/../secret",
		"references\\secret",
		"references/.hidden",
	] {
		e.config["files"][0]["path"] = json!(path);
		assert!(validate(&e).is_err(), "{path}");
	}
}
#[rstest::rstest]
fn skills_only_agents_do_not_need_custom_prompts() {
	let mut e = entry();
	e.kind = "agent".into();
	e.config = json!({"model":{"id":"model","version":"1.0.0"},"skills":[{"id":"research","version":"1.0.0"}]});
	assert!(validate(&e).is_ok());
	e.config["skills"] = json!([]);
	assert!(validate(&e).is_err());
	e.config["instructions"] = json!("Legacy instructions");
	assert!(validate(&e).is_ok());
}
#[rstest::rstest]
fn rejects_invalid_metadata() {
	let mut e = entry();
	e.version = "latest".into();
	assert!(validate(&e).is_err());
	e.version = "1.0.0".into();
	e.id = "../../escape".into();
	assert!(validate(&e).is_err());
}
#[rstest::rstest]
fn unknown_requirements_and_invalid_clusters_are_rejected() {
	for value in [
		json!({"capabilty":"web.search"}),
		json!({"capabilities":["web.search"]}),
	] {
		assert!(serde_json::from_value::<Search>(value).is_err());
	}
	let mut e = entry();
	e.kind = "cluster".into();
	for config in [
		json!({}),
		json!({"coordinator":"research"}),
		json!({"coordinator":{"id":"research","version":"latest"}}),
	] {
		e.config = config;
		assert!(validate(&e).is_err());
	}
	e.config = json!({"coordinator":{"id":"research","version":"1.0.0"}});
	validate(&e).unwrap();
}
#[rstest::rstest]
fn installation_overrides_cannot_clear_a_personal_agent_knowledge_digest() {
	let mut config = json!({"knowledge_digest":"immutable-digest"});
	assert!(overlay_config(&mut config, &json!({"knowledge_digest":null})).is_err());
	assert_eq!(config["knowledge_digest"], "immutable-digest");
	assert!(overlay_config(&mut config, &json!({"display_name":"Local name"})).is_ok());
	assert_eq!(config["display_name"], "Local name");
}
