use super::*;
use crate::{Error, ports::semantic::mutations::SemanticEntriesSession};
use aidash_domain::semantic::{
	EmbeddingConfig, Source, VectorConfig,
	indexing::IndexingSpec,
	mutations::{Entry, History, Index},
};
use async_trait::async_trait;
use chrono::Utc;
use rstest::rstest;
use serde_json::{Value, json};
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
			provider: "postgres".into(),
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

struct Scope {
	calls: Vec<&'static str>,
	denied: BTreeSet<String>,
	fail: Option<&'static str>,
	old: Option<Entry>,
	written: Option<Entry>,
	dependencies: Vec<(Uuid, i64)>,
	entries: Vec<Entry>,
	text: Option<String>,
	digest: Option<String>,
}
impl Default for Scope {
	fn default() -> Self {
		Self {
			calls: vec![],
			denied: BTreeSet::new(),
			fail: None,
			old: None,
			written: None,
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
