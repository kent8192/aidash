use super::*;
use crate::{Error, ports::semantic::mutations::SemanticEntriesSession};
use aidash_domain::semantic::{
	EmbeddingConfig, VectorConfig,
	indexing::IndexingSpec,
	mutations::{Entry, History, Index},
};
use async_trait::async_trait;
use chrono::Utc;
use rstest::rstest;
use std::collections::BTreeSet;

fn spec() -> IndexingSpec {
	IndexingSpec {
		embedding: EmbeddingConfig {
			provider: "openai".into(),
			endpoint: "https://provider.invalid".into(),
			credential_env: None,
			model: "fixture".into(),
			model_version: "1".into(),
			dimensions: 2,
		},
		vector: VectorConfig {
			provider: "qdrant".into(),
			endpoint: "https://vectors.invalid".into(),
			credential_env: None,
		},
		enabled: true,
		auto_context: true,
		max_sources: 8,
		max_results: 2,
		max_result_tokens: 128,
		max_input_bytes: 128,
	}
}
fn index() -> Index {
	Index {
		workspace_id: Uuid::from_u128(1),
		tenant: "tenant".into(),
		revision: 4,
		spec: serde_json::to_value(spec()).unwrap(),
		collection: "collection".into(),
		updated_at: Utc::now(),
	}
}
fn entry() -> Entry {
	Entry {
		id: Uuid::from_u128(2),
		workspace_id: Uuid::from_u128(1),
		key: "key".into(),
		source: json!({"kind":"memory","text":"content"}),
		agent: None,
		metadata: json!({}),
		revision: 1,
		point_id: Uuid::from_u128(3),
		index_revision: 4,
		deleted: false,
		state: "READY".into(),
		attempts: 0,
		last_error: None,
		created_by: "original".into(),
		updated_at: Utc::now(),
	}
}
fn run() -> aidash_domain::Run {
	aidash_domain::Run {
		id: Uuid::from_u128(10),
		task_id: Uuid::from_u128(11),
		workspace_id: index().workspace_id,
		home_node: "aidash://home".into(),
		agent_id: "agent".into(),
		agent_version: "1".into(),
		state_version: aidash_domain::StateVersion::default(),
		state: aidash_domain::RunState::default(),
		recovery: aidash_domain::RecoveryState::default(),
		control: aidash_domain::RunControl::Active,
		context: aidash_domain::context::Context::default(),
		step: 1,
		revision: 1,
		observed_input_seq: 0,
		ledger_worker_ready: true,
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: Utc::now(),
	}
}

struct Scope {
	calls: Vec<&'static str>,
	configured: bool,
	denied: BTreeSet<String>,
	fail: Option<&'static str>,
	old: Option<Entry>,
	written: Option<Entry>,
	binding: Option<(Entry, Uuid)>,
	plain_memory: Option<(Uuid, Value)>,
	dependencies: Vec<(Uuid, i64)>,
	entries: Vec<Entry>,
	text: Option<String>,
	digest: Option<String>,
}
impl Default for Scope {
	fn default() -> Self {
		Self {
			calls: vec![],
			configured: true,
			denied: BTreeSet::new(),
			fail: None,
			old: None,
			written: None,
			binding: None,
			plain_memory: None,
			dependencies: vec![],
			entries: vec![],
			text: Some("content".into()),
			digest: Some(content_digest("content")),
		}
	}
}
impl Scope {
	fn touch(&mut self, call: &'static str) -> Result<()> {
		self.calls.push(call);
		if self.fail == Some(call) {
			return Err(Error::Port(Box::new(std::io::Error::other(
				"scoped adapter fault",
			))));
		}
		Ok(())
	}
}
#[async_trait]
impl SemanticEntriesSession for Scope {
	async fn workspace(&mut self, _workspace: Uuid, action: &str) -> Result<()> {
		self.touch("workspace")?;
		if self.denied.contains(action) {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn index(&mut self, _workspace: Uuid, exclusive: bool) -> Result<Index> {
		self.touch(if exclusive {
			"index_update"
		} else {
			"index_share"
		})?;
		Ok(index())
	}
	async fn by_key(&mut self, _workspace: Uuid, _key: &str) -> Result<Option<Entry>> {
		self.touch("by_key")?;
		Ok(self.old.clone())
	}
	fn saved(&mut self) -> Result<Value> {
		self.touch("saved")?;
		Ok(json!({"subject":"worker"}))
	}
	async fn permits(&mut self, _entry: &Entry, action: &str) -> Result<bool> {
		self.touch(if action == "semantic.read" {
			"read"
		} else {
			"write"
		})?;
		Ok(!self.denied.contains(action))
	}
	async fn source(&mut self, _workspace: Uuid, _source: &Source) -> Result<Option<String>> {
		self.touch("source")?;
		Ok(self.text.clone())
	}
	async fn count(&mut self, _workspace: Uuid) -> Result<i64> {
		self.touch("count")?;
		Ok(0)
	}
	async fn put(&mut self, entry: Entry, _authority: Value) -> Result<Entry> {
		self.touch("put")?;
		self.written = Some(entry.clone());
		Ok(entry)
	}
	async fn schedule(&mut self, _entry: &Entry, _collection: &str) -> Result<()> {
		self.touch("schedule")
	}
	async fn history(
		&mut self,
		_workspace: Uuid,
		_id: Uuid,
		_revision: i64,
		_state: &str,
		_detail: &str,
	) -> Result<()> {
		self.touch("history")
	}
	async fn list(&mut self, _workspace: Uuid) -> Result<Vec<Entry>> {
		panic!("unexpected entry operation: list")
	}
	async fn lock_entry(&mut self, _workspace: Uuid, _id: Uuid) -> Result<Option<Entry>> {
		panic!("unexpected entry operation: lock_entry")
	}
	async fn reindex_replay(&mut self, _id: Uuid, _revision: i64) -> Result<bool> {
		panic!("unexpected entry operation: reindex_replay")
	}
	async fn managed_memory(&mut self, _id: Uuid) -> Result<Option<(String, String, String)>> {
		panic!("unexpected entry operation: managed_memory")
	}
	async fn require_memory_write(
		&mut self,
		_workspace: Uuid,
		_agent: &str,
		_version: &str,
	) -> Result<()> {
		panic!("unexpected entry operation: require_memory_write")
	}
	async fn delete_memory(
		&mut self,
		_workspace: Uuid,
		_agent: String,
		_version: String,
		_home: String,
	) -> Result<()> {
		panic!("unexpected entry operation: delete_memory")
	}
	async fn change(
		&mut self,
		_workspace: Uuid,
		_id: Uuid,
		_point: Uuid,
		_index_revision: i64,
		_delete: bool,
		_authority: Value,
	) -> Result<Entry> {
		panic!("unexpected entry operation: change")
	}
	async fn history_page(&mut self, _workspace: Uuid, _cursor: i64) -> Result<Vec<History>> {
		panic!("unexpected entry operation: history_page")
	}
	async fn history_entry(&mut self, _id: Uuid) -> Result<Entry> {
		panic!("unexpected entry operation: history_entry")
	}
}
#[async_trait]
impl SemanticMemoryReadSession for Scope {
	async fn permits(&mut self, entry: &Entry, action: &str) -> Result<bool> {
		SemanticEntriesSession::permits(self, entry, action).await
	}
	async fn source(&mut self, workspace: Uuid, source: &Source) -> Result<Option<String>> {
		SemanticEntriesSession::source(self, workspace, source).await
	}

	async fn dependencies(&mut self, _: Uuid) -> Result<Vec<(Uuid, i64)>> {
		self.touch("dependencies")?;
		Ok(self.dependencies.clone())
	}
	async fn entry(&mut self, id: Uuid) -> Result<Option<Entry>> {
		self.touch("entry")?;
		Ok(self.entries.iter().find(|entry| entry.id == id).cloned())
	}
	async fn point_digest(&mut self, _: Uuid) -> Result<Option<String>> {
		self.touch("digest")?;
		Ok(self.digest.clone())
	}
}
#[async_trait]
impl SemanticMemoryWriteSession for Scope {
	async fn configured(&mut self, _: Uuid) -> Result<bool> {
		self.touch("configured")?;
		Ok(self.configured)
	}
	async fn revision(&mut self, _: Uuid, _: &str) -> Result<Option<i64>> {
		self.touch("revision")?;
		Ok(self.old.as_ref().map(|entry| entry.revision))
	}
	async fn bind_memory(&mut self, entry: &Entry, run: &Run) -> Result<()> {
		self.touch("bind_memory")?;
		self.binding = Some((entry.clone(), run.id));
		Ok(())
	}
	async fn persist_memory(&mut self, run: &Run, data: &Value) -> Result<()> {
		self.touch("persist_memory")?;
		self.plain_memory = Some((run.id, data.clone()));
		Ok(())
	}
}

#[rstest]
#[tokio::test]
async fn unconfigured_semantic_index_still_persists_ordinary_memory() {
	let mut scope = Scope {
		configured: false,
		..Default::default()
	};
	remember(&mut scope, &run(), &json!({"note":"content"}))
		.await
		.unwrap();
	assert_eq!(scope.calls, ["configured", "persist_memory"]);
	assert_eq!(
		scope.plain_memory,
		Some((run().id, json!({"note":"content"})))
	);
	assert!(scope.written.is_none());
	assert!(scope.binding.is_none());
}

#[rstest]
#[tokio::test]
async fn managed_memory_uses_the_qualified_agent_and_current_revision() {
	let run = run();
	let data = json!({"note":"content"});
	let mut scope = Scope::default();
	remember(&mut scope, &run, &data).await.unwrap();
	let written = scope.written.unwrap();
	let bound = scope.binding.unwrap();
	let agent = qualified_agent(&run.home_node, &run.agent_id, &run.agent_version);
	assert_eq!(
		written.key,
		format!("agent-memory:{}", content_digest(&agent))
	);
	assert_eq!(written.agent, Some(agent));
	assert_eq!(written.workspace_id, run.workspace_id);
	assert_eq!(
		written.source,
		json!({"kind":"memory","text":serde_json::to_string(&data).unwrap()})
	);
	assert_eq!(written.metadata, json!({"origin":"agent_memory"}));
	assert_eq!(written.revision, 1);
	assert_eq!(bound.0.id, written.id);
	assert_eq!(bound.1, run.id);
	assert_eq!(
		scope.calls,
		[
			"configured",
			"index_update",
			"revision",
			"workspace",
			"index_update",
			"by_key",
			"saved",
			"count",
			"write",
			"read",
			"source",
			"put",
			"schedule",
			"history",
			"bind_memory",
			"persist_memory"
		]
	);
}

#[rstest]
#[tokio::test]
async fn existing_memory_advances_its_stored_revision_before_binding() {
	let mut old = entry();
	old.key = format!(
		"agent-memory:{}",
		content_digest(&qualified_agent(
			&run().home_node,
			&run().agent_id,
			&run().agent_version
		))
	);
	old.revision = 7;
	old.agent = Some("previous".into());
	let mut scope = Scope {
		old: Some(old.clone()),
		..Default::default()
	};
	remember(&mut scope, &run(), &json!({"note":"new"}))
		.await
		.unwrap();
	let written = scope.written.unwrap();
	assert_eq!(written.id, old.id);
	assert_eq!(written.revision, 8);
	assert_eq!(written.created_by, "original");
	assert_eq!(scope.binding.unwrap().0.revision, 8);
}

#[rstest]
#[case::workspace("semantic.write")]
#[case::source("semantic.read")]
#[tokio::test]
async fn denied_memory_never_writes_or_binds(#[case] action: &str) {
	let mut scope = Scope {
		denied: BTreeSet::from([action.into()]),
		..Default::default()
	};
	assert!(matches!(
		remember(&mut scope, &run(), &json!({})).await,
		Err(Error::Forbidden)
	));
	assert!(scope.written.is_none());
	assert!(scope.binding.is_none());
	assert!(scope.plain_memory.is_none());
	assert!(!scope.calls.contains(&"bind_memory"));
}

#[rstest]
#[case::configuration("configured")]
#[case::index_lock("index_update")]
#[case::revision("revision")]
#[case::write("put")]
#[case::schedule("schedule")]
#[case::history("history")]
#[case::binding("bind_memory")]
#[case::ordinary_memory("persist_memory")]
#[tokio::test]
async fn memory_preserves_adapter_error_identity(#[case] operation: &'static str) {
	let mut scope = Scope {
		fail: Some(operation),
		..Default::default()
	};
	let Err(Error::Port(error)) = remember(&mut scope, &run(), &json!({})).await else {
		panic!("expected original port error")
	};
	assert!(error.downcast_ref::<std::io::Error>().is_some());
	assert_eq!(scope.calls.last(), Some(&operation));
}

#[rstest]
#[case::empty(false)]
#[case::complete(true)]
#[tokio::test]
async fn unchanged_dependencies_remain_visible(#[case] dependency: bool) {
	let entry = entry();
	let mut scope = Scope::default();
	if dependency {
		scope.dependencies.push((entry.id, entry.revision));
		scope.entries.push(entry);
	}
	assert!(reads_visible(&mut scope, Uuid::from_u128(7)).await.unwrap());
	assert_eq!(
		scope.calls,
		if dependency {
			vec!["dependencies", "entry", "read", "source", "digest"]
		} else {
			vec!["dependencies"]
		}
	);
}

#[rstest]
#[case::deleted("deleted")]
#[case::revision("revision")]
#[case::revoked("revoked")]
#[case::source_removed("source")]
#[case::source_changed("digest")]
#[case::missing_digest("missing_digest")]
#[tokio::test]
async fn changed_dependencies_prevent_content_delivery(#[case] change: &str) {
	let entry = entry();
	let mut scope = Scope {
		dependencies: vec![(entry.id, entry.revision)],
		entries: vec![entry],
		..Default::default()
	};
	match change {
		"deleted" => scope.entries[0].deleted = true,
		"revision" => scope.entries[0].revision += 1,
		"revoked" => {
			scope.denied.insert("semantic.read".into());
		}
		"source" => scope.text = None,
		"digest" => scope.text = Some("changed content".into()),
		"missing_digest" => scope.digest = None,
		_ => unreachable!(),
	}
	assert!(!reads_visible(&mut scope, Uuid::from_u128(7)).await.unwrap());
	if matches!(change, "deleted" | "revision") {
		assert_eq!(scope.calls, ["dependencies", "entry"]);
	}
	if change == "revoked" {
		assert_eq!(scope.calls, ["dependencies", "entry", "read"]);
	}
}

#[rstest]
#[tokio::test]
async fn corrupt_persisted_source_is_a_storage_error() {
	let mut entry = entry();
	entry.source = json!({"kind":"invalid"});
	let mut scope = Scope {
		dependencies: vec![(entry.id, entry.revision)],
		entries: vec![entry],
		..Default::default()
	};
	assert!(matches!(
		reads_visible(&mut scope, Uuid::from_u128(7)).await,
		Err(Error::Json(_))
	));
	assert_eq!(scope.calls, ["dependencies", "entry", "read"]);
}

#[rstest]
#[tokio::test]
async fn every_dependency_is_checked_before_content_is_visible() {
	let first = entry();
	let mut second = first.clone();
	second.id = Uuid::from_u128(8);
	second.revision = 2;
	let mut scope = Scope {
		dependencies: vec![(first.id, first.revision), (second.id, 1)],
		entries: vec![first, second],
		..Default::default()
	};
	assert!(!reads_visible(&mut scope, Uuid::from_u128(7)).await.unwrap());
	assert_eq!(
		scope.calls,
		["dependencies", "entry", "read", "source", "digest", "entry"]
	);
}

#[rstest]
#[tokio::test]
async fn missing_native_dependency_denies_without_policy_or_digest() {
	let mut scope = Scope {
		dependencies: vec![(Uuid::from_u128(999), 1)],
		..Default::default()
	};
	assert!(
		!reads_visible(&mut scope, Uuid::from_u128(10))
			.await
			.unwrap()
	);
	assert_eq!(scope.calls, vec!["dependencies", "entry"]);
}
