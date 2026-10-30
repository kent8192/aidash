//! Source eligibility is tested from real admitted closures, independently of
//! the Harness's provider-neutral request rendering tests.
use super::*;
use aidash_application::ports::bindings::{BindingCatalog, StoredAgentPlugin};
use aidash_domain::registry::{Entry, Projection, bindings::BindingSnapshot};

struct Catalog(Vec<Entry>, Option<StoredAgentPlugin>);
#[async_trait]
impl BindingCatalog for Catalog {
	async fn agent_plugin(&mut self, digest: &str) -> Result<Option<StoredAgentPlugin>> {
		Ok(self
			.1
			.clone()
			.filter(|stored| stored.plugin.digest == digest))
	}
	async fn definition(&mut self, reference: &QualifiedRef) -> Result<Entry> {
		self.0
			.iter()
			.find(|entry| entry.id == reference.id && entry.version == reference.version)
			.cloned()
			.ok_or_else(|| aidash_application::Error::NotFound(reference.id.clone()))
	}
	async fn installation(&mut self, _: &Projection) -> Result<()> {
		Err(aidash_application::Error::Forbidden)
	}
	async fn source(&mut self, _: &Entry) -> Result<()> {
		Ok(())
	}
}

async fn admitted(source: Value) -> BindingSnapshot {
	admitted_with(Some(source), None, false).await
}

async fn admitted_with(
	source: Option<Value>,
	plugin: Option<StoredAgentPlugin>,
	deferred: bool,
) -> BindingSnapshot {
	const NODE: &str = "aidash://source-plan-test";
	let entry = |id: &str, kind: &str, config: Value| -> Entry {
		serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"Source plan fixture"},"config":config})).unwrap()
	};
	let source = source.map(|source| {
		entry(
			"references",
			"source",
			json!({"schema_version":1,"source":source}),
		)
	});
	let mut agent = entry(
		"reader",
		"agent",
		json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"Read the admitted reference sources.","bindings":[{"kind":"source","target":{"registry_deployment":NODE,"id":"references","version":"1.0.0"},"narrow":{}}],"remove_default":["compaction"]}),
	);
	if source.is_none() {
		agent.config["bindings"] = json!([]);
	}
	if let Some(plugin) = &plugin {
		agent.config["agent_plugins"] = json!([aidash_domain::agent_plugin::AgentPluginRef::of(
			&plugin.plugin,
			&plugin.source
		)]);
	}
	if deferred {
		agent.config["exposure"] = json!({"version":"deferred@1"});
	}
	agent.normalize_agent(NODE).unwrap();
	let validation = crate::bootstrap::registry_validation();
	let mut entries =
		aidash_application::registry::system::declarations(&validation, NODE).unwrap();
	entries.extend(source);
	entries.push(entry("model", "model", json!({"provider":"openrouter","model_id":"fixture","endpoint":"https://models.invalid","context_window":32768,"max_output_tokens":1024,"modalities":["text"],"cost":{}})));
	let snapshot = aidash_application::registry::bindings::resolve(
		&mut Catalog(entries, plugin),
		&validation,
		QualifiedRef {
			registry_deployment: NODE.into(),
			id: agent.id.clone(),
			version: agent.version.clone(),
		},
		&agent,
		false,
	)
	.await
	.unwrap();
	snapshot.validate().unwrap();
	snapshot
}

#[rstest::rstest]
#[case::legacy(false)]
#[case::deferred(true)]
#[tokio::test]
async fn plugin_only_skills_have_an_instruction_source(#[case] deferred: bool) {
	use aidash_domain::agent_plugin::{PLUGIN_SCHEMA, PluginFile};
	let file = |path: &str, text: String| PluginFile {
		path: path.into(),
		bytes: text.into_bytes(),
	};
	let plugin = aidash_domain::agent_plugin::load(&[
		file(
			"plugin.json",
			json!({"$schema":PLUGIN_SCHEMA,"name":"acme-skills","version":"1.0.0"}).to_string(),
		),
		file(
			"skills/lint/SKILL.md",
			"---\nname: lint\ndescription: Review code.\n---\nCheck the code carefully.\n".into(),
		),
	])
	.unwrap();
	let snapshot = admitted_with(
		None,
		Some(StoredAgentPlugin {
			source: "https://github.com/acme/tools/tree/0123456789abcdef0123456789abcdef01234567"
				.into(),
			plugin,
		}),
		deferred,
	)
	.await;
	let plan = compile_context_source_plan(Uuid::from_u128(4), &snapshot, true, false).unwrap();
	let instructions = plan
		.iter()
		.filter(|source| source.target == ContextSourceTarget::Instructions)
		.collect::<Vec<_>>();
	assert_eq!(instructions.len(), 1);
	assert_eq!(
		instructions[0].contribution_slot,
		if deferred {
			"resident_skills"
		} else {
			"skills"
		}
	);
}

#[tokio::test]
async fn reference_mounts_do_not_create_automatic_document_fields() {
	let attachment = admitted(json!({"adapter":"reference_attachments","references":[{"reference_id":Uuid::from_u128(1),"digest":"a".repeat(64)}]})).await;
	let private = admitted(json!({"adapter":"private_references","digest":"b".repeat(64)})).await;
	let documents = ContextSourceTarget::Pinned("reference_documents".into());
	let plan = compile_context_source_plan(Uuid::from_u128(2), &attachment, true, false).unwrap();
	assert!(plan.iter().all(|source| source.target != documents));
	// Private documents have always retained their fixed field, including an
	// empty document list. Mount-only declarations must not manufacture it.
	let plan = compile_context_source_plan(Uuid::from_u128(3), &private, true, false).unwrap();
	assert_eq!(
		plan.iter()
			.filter(|source| source.target == documents)
			.count(),
		1
	);
	assert_eq!(
		aidash_domain::registry::AgentConfig::from_snapshot(&attachment)
			.unwrap()
			.reference_attachments
			.len(),
		1,
		"manual reference mounts remain admitted"
	);
}

#[tokio::test]
async fn pinned_reference_images_have_a_separate_uncached_context_section() {
	assert!(Slot::ReferenceImages.requires_mandatory_delivery(false));
	assert!(Slot::ReferenceImages.requires_mandatory_delivery(true));
	assert!(!Slot::Semantic.requires_mandatory_delivery(false));
	let mut snapshot =
		admitted(json!({"adapter":"private_references","digest":"b".repeat(64)})).await;
	let agent = snapshot
		.definitions
		.iter_mut()
		.find(|definition| definition.identity == snapshot.agent)
		.unwrap();
	agent.definition.config["reference_embedding"] = json!({"id":"embed","version":"1.0.0"});
	let plan = compile_context_source_plan(Uuid::from_u128(5), &snapshot, true, false).unwrap();
	let image = plan
		.iter()
		.find(|source| source.contribution_slot == "reference_images")
		.unwrap();
	assert_eq!(
		image.target,
		ContextSourceTarget::Pinned("reference_images".into())
	);
	assert_eq!(image.phase, ContextSourcePhase::BudgetedData);
	assert_eq!(image.cache, ContextSourceCache::None);
	assert!(image.refresh_each_step);
	assert!(
		plan.iter().any(
			|source| source.target == ContextSourceTarget::Pinned("reference_documents".into())
		)
	);
}
