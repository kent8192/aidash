use super::*;
use crate::{
	authorization::source::semantic as use_case,
	ports::authorization::source::semantic::SemanticBindingScope,
};
use aidash_domain::{
	generation::remote::Ancestor,
	registry::rules::digest,
	semantic::{
		Failure,
		indexing::IndexingSpec,
		mutations::Index,
		remote::{Binding, Request, VERSION},
	},
};
use chrono::{TimeZone, Utc};
struct BindingScope {
	base: Scope,
	index: Index,
	spec: IndexingSpec,
	entry: Entry,
	exists: bool,
	reads: Vec<&'static str>,
}
impl BindingScope {
	fn new() -> Self {
		let embedding = json!({"provider":"openai","endpoint":"https://embedding.invalid","credential_env":"EMBEDDING","model":"embed","model_version":"1","dimensions":3});
		let spec:IndexingSpec=serde_json::from_value(json!({"embedding":embedding,"vector":{"provider":"qdrant","endpoint":"https://vector.invalid","credential_env":null},"enabled":true,"auto_context":true,"max_sources":100,"max_results":4,"max_result_tokens":1024,"max_input_bytes":32768})).unwrap();
		let mut entry = entry("embedding");
		entry.config = embedding;
		Self {
			base: Scope::new(),
			index: Index {
				workspace_id: task().workspace_id,
				tenant: "tenant".into(),
				revision: 9,
				spec: serde_json::to_value(&spec).unwrap(),
				collection: "collection".into(),
				updated_at: Utc.timestamp_opt(1, 0).unwrap(),
			},
			spec,
			entry,
			exists: true,
			reads: vec![],
		}
	}
}
#[async_trait]
impl SourceAuthorityScope for BindingScope {
	fn source_subjects(&self) -> &[String] {
		self.base.source_subjects()
	}
	fn source_bundle(&self) -> &PolicyBundle {
		self.base.source_bundle()
	}
	fn source_context(&mut self, attributes: Value) {
		self.base.source_context(attributes)
	}
	fn source_resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.base.source_resource(kind, id, attributes)
	}
	async fn generation_home(
		&mut self,
		task: &Task,
		node: &str,
		generation: Option<&Value>,
	) -> Result<()> {
		self.base.generation_home(task, node, generation).await
	}
	async fn source_workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.base.source_workspace(id).await
	}
	async fn source_task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.base.source_task_resource(task).await
	}
	async fn source_require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.base.source_require(resource, action).await
	}
}
#[async_trait]
impl SemanticBindingScope for BindingScope {
	fn home_node_id(&self) -> &str {
		"aidash://home"
	}
	fn binding_tenant(&self) -> &str {
		"tenant"
	}
	async fn semantic_index(&mut self, _: Uuid) -> Result<(Index, IndexingSpec)> {
		self.reads.push("index");
		if self.exists {
			Ok((self.index.clone(), self.spec.clone()))
		} else {
			Err(Error::NotFound("semantic index".into()))
		}
	}
	async fn embedding_entry(&mut self, _: &EntityRef, action: &str) -> Result<Entry> {
		self.reads.push("entry");
		let resource = self.source_resource(&self.entry.kind, &self.entry.id, json!({}));
		self.source_require(&resource, action).await?;
		Ok(self.entry.clone())
	}
	async fn binding_lineage(&mut self) -> Result<Vec<Ancestor>> {
		self.reads.push("lineage");
		Ok(vec![])
	}
}
fn remote_inspection() -> Inspection {
	let mut value = inspection("model");
	value.agent.config = json!({"model":{"id":"model","version":"1.0.0"}});
	value.semantic_memory = VERSION;
	value
}
fn request() -> Request {
	Request::RequiredHome {
		embedding: EntityRef {
			id: "embedding".into(),
			version: "1.0.0".into(),
		},
		compactor: None,
	}
}
#[rstest]
#[tokio::test]
async fn disabled_binding_does_not_read_or_disclose_home_state() {
	let mut scope = BindingScope::new();
	assert_eq!(
		use_case::binding(
			&mut scope,
			&task(),
			"aidash://receiver",
			&inspection("model"),
			&Request::Disabled {}
		)
		.await
		.unwrap(),
		Binding::Disabled {}
	);
	assert!(scope.base.calls.is_empty());
	assert!(scope.reads.is_empty());
}
#[rstest]
#[case("version")]
#[case("compactor")]
#[case("disabled_agent_memory")]
#[tokio::test]
async fn unsupported_inspection_or_agent_memory_is_rejected_before_source_reads(
	#[case] change: &str,
) {
	let mut scope = BindingScope::new();
	let mut inspection = remote_inspection();
	match change {
		"version" => inspection.semantic_memory += 1,
		"compactor" => {
			inspection.compactor = Some(EntityRef {
				id: "compactor".into(),
				version: "1.0.0".into(),
			})
		}
		"disabled_agent_memory" => {
			inspection.agent.config["allow_cross_conversation_memory"] = json!(false);
			inspection.agent.config["allow_workspace_retrieval"] = json!(false);
		}
		_ => panic!("unknown contract"),
	}
	assert!(matches!(
		use_case::binding(
			&mut scope,
			&task(),
			"aidash://receiver",
			&inspection,
			&request()
		)
		.await,
		Err(Error::RemoteSemantic(Failure::Configuration))
	));
	assert!(scope.base.calls.is_empty());
	assert!(scope.reads.is_empty());
}
#[rstest]
#[tokio::test]
async fn disclosure_permission_is_required_before_index_or_provider_reads() {
	let mut scope = BindingScope::new();
	scope.base.denied = Some("semantic.disclose");
	assert!(matches!(
		use_case::binding(
			&mut scope,
			&task(),
			"aidash://receiver",
			&remote_inspection(),
			&request()
		)
		.await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.base.calls.last().unwrap().0, "semantic.disclose");
	assert!(scope.reads.is_empty());
	let (_, resource, _) = scope.base.calls.last().unwrap();
	assert_eq!(
		resource.attributes["remote_node"],
		json!("aidash://receiver")
	);
	assert_eq!(
		resource.attributes["inference_model"],
		json!({"id":"model","version":"1.0.0"})
	);
}
#[rstest]
#[case("missing")]
#[case("tenant")]
#[case("disabled")]
#[case("auto_context")]
#[tokio::test]
async fn home_index_must_exist_and_enable_the_same_tenant_context(#[case] change: &str) {
	let mut scope = BindingScope::new();
	match change {
		"missing" => scope.exists = false,
		"tenant" => scope.index.tenant = "other".into(),
		"disabled" => scope.spec.enabled = false,
		"auto_context" => scope.spec.auto_context = false,
		_ => panic!("unknown index"),
	}
	assert!(matches!(
		use_case::binding(
			&mut scope,
			&task(),
			"aidash://receiver",
			&remote_inspection(),
			&request()
		)
		.await,
		Err(Error::RemoteSemantic(Failure::Configuration))
	));
	assert_eq!(scope.reads, vec!["index"]);
}
#[rstest]
#[case("kind")]
#[case("configuration")]
#[tokio::test]
async fn embedding_definition_must_match_the_locked_index_configuration(#[case] change: &str) {
	let mut scope = BindingScope::new();
	if change == "kind" {
		scope.entry.kind = "model".into();
	} else {
		scope.entry.config["model"] = json!("changed");
	}
	assert!(matches!(
		use_case::binding(
			&mut scope,
			&task(),
			"aidash://receiver",
			&remote_inspection(),
			&request()
		)
		.await,
		Err(Error::RemoteSemantic(Failure::Configuration))
	));
	assert_eq!(scope.reads, vec!["index", "entry"]);
}
#[rstest]
#[tokio::test]
async fn required_home_binding_pins_the_exact_index_document_and_provider_metadata() {
	let mut scope = BindingScope::new();
	let index_digest = digest(&scope.index.spec);
	let provider_digest = digest(&serde_json::to_value(&scope.entry).unwrap());
	let config_digest = digest(&scope.entry.config);
	let result = use_case::binding(
		&mut scope,
		&task(),
		"aidash://receiver",
		&remote_inspection(),
		&request(),
	)
	.await
	.unwrap();
	let Binding::RequiredHome {
		version,
		index_revision,
		index_digest: actual,
		embedding,
		compactor,
		..
	} = result
	else {
		panic!("required Home binding expected")
	};
	assert_eq!(version, VERSION);
	assert_eq!(index_revision, 9);
	assert_eq!(actual, index_digest);
	assert_eq!(embedding.node_id, "aidash://home");
	assert_eq!(embedding.digest, provider_digest);
	assert_eq!(embedding.configuration_digest, config_digest);
	assert_eq!(compactor, None);
	assert_eq!(scope.reads, vec!["index", "entry", "lineage"]);
}
#[rstest]
#[tokio::test]
async fn compactor_binding_requires_the_pinned_remote_definition() {
	let mut scope = BindingScope::new();
	let mut inspection = remote_inspection();
	let compactor = EntityRef {
		id: "compactor".into(),
		version: "1.0.0".into(),
	};
	inspection.compactor = Some(compactor.clone());
	let request = Request::RequiredHome {
		embedding: EntityRef {
			id: "embedding".into(),
			version: "1.0.0".into(),
		},
		compactor: Some(compactor),
	};
	assert!(matches!(
		use_case::binding(
			&mut scope,
			&task(),
			"aidash://receiver",
			&inspection,
			&request
		)
		.await,
		Err(Error::RemoteSemantic(Failure::Configuration))
	));
	assert_eq!(scope.reads, vec!["index", "entry"]);
}
