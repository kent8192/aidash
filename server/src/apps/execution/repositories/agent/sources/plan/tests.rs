//! Source eligibility is tested from real admitted closures, independently of
//! the operator's provider-neutral request rendering tests.
use super::*;
use aidash_application::ports::bindings::BindingCatalog;
use aidash_domain::registry::{Entry, Projection, bindings::BindingSnapshot};

struct Catalog(Vec<Entry>);
#[async_trait]
impl BindingCatalog for Catalog {
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
	const NODE: &str = "aidash://source-plan-test";
	let entry = |id: &str, kind: &str, config: Value| -> Entry {
		serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"Source plan fixture"},"config":config})).unwrap()
	};
	let source = entry(
		"references",
		"source",
		json!({"schema_version":1,"source":source}),
	);
	let mut agent = entry(
		"reader",
		"agent",
		json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"bindings":[{"kind":"source","target":{"registry_deployment":NODE,"id":"references","version":"1.0.0"},"narrow":{}}],"remove_default":["compaction"]}),
	);
	agent.normalize_agent(NODE).unwrap();
	let validation = crate::bootstrap::registry_validation();
	let mut entries =
		aidash_application::registry::system::declarations(&validation, NODE).unwrap();
	entries.extend([source, entry("model", "model", json!({"provider":"openrouter","model_id":"fixture","endpoint":"https://models.invalid","context_window":32768,"max_output_tokens":1024,"modalities":["text"],"cost":{}}))]);
	let snapshot = aidash_application::registry::bindings::resolve(
		&mut Catalog(entries),
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
