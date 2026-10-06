use super::*;
use aidash_domain::semantic::{EmbeddingConfig, Source, VectorConfig};
use async_trait::async_trait;
use rstest::rstest;
use serde_json::{Value, json};
use std::collections::{BTreeSet, VecDeque};

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
fn input() -> Put {
	Put {
		key: "key".into(),
		expected_revision: 0,
		source: Source::Memory {
			text: "content".into(),
		},
		agent: None,
		metadata: json!({}),
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
	fail: Option<&'static str>,
	denied: BTreeSet<String>,
	deny_entries: bool,
	cursors: Vec<i64>,
	old: Option<Entry>,
	rows: Vec<Entry>,
	count: i64,
	text: Option<String>,
	replay: bool,
	managed: bool,
	pages: VecDeque<Vec<History>>,
}
impl Default for Scope {
	fn default() -> Self {
		Self {
			calls: vec![],
			fail: None,
			denied: BTreeSet::new(),
			deny_entries: false,
			cursors: vec![],
			old: None,
			rows: vec![],
			count: 0,
			text: Some("content".into()),
			replay: false,
			managed: false,
			pages: VecDeque::new(),
		}
	}
}
impl Scope {
	fn touch(&mut self, name: &'static str) -> Result<()> {
		self.calls.push(name);
		if self.fail == Some(name) {
			return Err(Error::External(name.into()));
		}
		Ok(())
	}
}
#[async_trait]
impl SemanticEntriesSession for Scope {
	async fn workspace(&mut self, _: Uuid, action: &str) -> Result<()> {
		self.touch("workspace")?;
		if self.denied.contains(action) {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn index(&mut self, _: Uuid, exclusive: bool) -> Result<Index> {
		self.touch(if exclusive {
			"index_update"
		} else {
			"index_share"
		})?;
		Ok(index())
	}
	async fn by_key(&mut self, _: Uuid, _: &str) -> Result<Option<Entry>> {
		self.touch("by_key")?;
		Ok(self.old.clone())
	}
	fn saved(&mut self) -> Result<Value> {
		self.touch("saved")?;
		Ok(json!({"subject":"current"}))
	}
	async fn permits(&mut self, _: &Entry, action: &str) -> Result<bool> {
		self.touch(if action == "semantic.read" {
			"read"
		} else {
			"write"
		})?;
		Ok(!self.deny_entries && !self.denied.contains(action))
	}
	async fn source(&mut self, _: Uuid, _: &Source) -> Result<Option<String>> {
		self.touch("source")?;
		Ok(self.text.clone())
	}
	async fn count(&mut self, _: Uuid) -> Result<i64> {
		self.touch("count")?;
		Ok(self.count)
	}
	async fn put(&mut self, entry: Entry, _: Value) -> Result<Entry> {
		self.touch("put")?;
		Ok(entry)
	}
	async fn schedule(&mut self, _: &Entry, _: &str) -> Result<()> {
		self.touch("schedule")
	}
	async fn history(&mut self, _: Uuid, _: Uuid, _: i64, _: &str, _: &str) -> Result<()> {
		self.touch("history")
	}
	async fn list(&mut self, _: Uuid) -> Result<Vec<Entry>> {
		self.touch("list")?;
		Ok(self.rows.clone())
	}
	async fn lock_entry(&mut self, _: Uuid, _: Uuid) -> Result<Option<Entry>> {
		self.touch("lock_entry")?;
		Ok(self.old.clone())
	}
	async fn reindex_replay(&mut self, _: Uuid, _: i64) -> Result<bool> {
		self.touch("reindex_replay")?;
		Ok(self.replay)
	}
	async fn managed_memory(&mut self, _: Uuid) -> Result<Option<(String, String, String)>> {
		self.touch("managed")?;
		Ok(self
			.managed
			.then(|| ("agent".into(), "1".into(), "home".into())))
	}
	async fn require_memory_write(&mut self, _: Uuid, _: &str, _: &str) -> Result<()> {
		self.touch("memory_write")
	}
	async fn delete_memory(&mut self, _: Uuid, _: String, _: String, _: String) -> Result<()> {
		self.touch("delete_memory")
	}
	async fn change(
		&mut self,
		_: Uuid,
		_: Uuid,
		point: Uuid,
		revision: i64,
		delete: bool,
		_: Value,
	) -> Result<Entry> {
		self.touch("change")?;
		let mut entry = self.old.clone().unwrap();
		entry.revision += 1;
		entry.point_id = point;
		entry.index_revision = revision;
		entry.deleted = delete;
		entry.state = if delete { "DELETED" } else { "PENDING" }.into();
		Ok(entry)
	}
	async fn history_page(&mut self, _: Uuid, cursor: i64) -> Result<Vec<History>> {
		self.touch("history_page")?;
		self.cursors.push(cursor);
		Ok(self.pages.pop_front().unwrap_or_default())
	}
	async fn history_entry(&mut self, _: Uuid) -> Result<Entry> {
		self.touch("history_entry")?;
		Ok(entry())
	}
}

#[rstest]
#[tokio::test]
async fn new_sources_share_one_authorized_write_and_history_order() {
	let mut scope = Scope::default();
	let result = put(&mut scope, Uuid::from_u128(1), input()).await.unwrap();
	assert_eq!(result.revision, 1);
	assert_eq!(result.created_by, "current");
	assert_eq!(result.state, "PENDING");
	assert_eq!(
		scope.calls,
		[
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
			"history"
		]
	);
}
#[rstest]
#[case::zero(0)]
#[case::current(1)]
#[tokio::test]
async fn identical_inputs_replay_after_authority_without_new_points(#[case] revision: i64) {
	let old = entry();
	let mut scope = Scope {
		old: Some(old.clone()),
		..Default::default()
	};
	let mut input = input();
	input.expected_revision = revision;
	let result = put(&mut scope, old.workspace_id, input).await.unwrap();
	assert_eq!(result.point_id, old.point_id);
	assert_eq!(result.created_by, "original");
	assert_eq!(result.state, "READY");
	assert_eq!(
		scope.calls,
		[
			"workspace",
			"index_update",
			"by_key",
			"saved",
			"write",
			"read"
		]
	);
}
#[rstest]
#[case::deleted(true, false)]
#[case::origin(false, true)]
#[tokio::test]
async fn authority_precedes_tombstone_or_source_decoding(
	#[case] deleted: bool,
	#[case] invalid: bool,
) {
	let mut old = entry();
	old.deleted = deleted;
	if invalid {
		old.source = json!({"untrusted":"invalid"});
	}
	let mut scope = Scope {
		old: Some(old),
		fail: Some("write"),
		..Default::default()
	};
	let error = put(&mut scope, Uuid::from_u128(1), input())
		.await
		.unwrap_err();
	assert_eq!(error.to_string(), "write");
	assert_eq!(scope.calls.last(), Some(&"write"));
	assert!(!scope.calls.contains(&"put"));
}
#[rstest]
#[case::workspace("workspace")]
#[case::index("index_update")]
#[case::key("by_key")]
#[case::authority("saved")]
#[case::count("count")]
#[case::write("write")]
#[case::read("read")]
#[case::source("source")]
#[case::write_database("put")]
#[case::point("schedule")]
#[case::history("history")]
#[tokio::test]
async fn port_failures_stop_the_mutation_without_later_effects(#[case] failure: &'static str) {
	let mut scope = Scope {
		fail: Some(failure),
		..Default::default()
	};
	let error = put(&mut scope, Uuid::from_u128(1), input())
		.await
		.unwrap_err();
	assert_eq!(error.to_string(), failure);
	assert_eq!(scope.calls.last(), Some(&failure));
}
#[rstest]
#[case::count(8, Some("content".into()), "semantic source limit reached")]
#[case::missing(0, None, "forbidden")]
#[case::bytes(0, Some("a".repeat(129)), "semantic text is empty or exceeds the configured byte limit")]
#[tokio::test]
async fn bounds_and_unavailable_sources_do_not_persist(
	#[case] count: i64,
	#[case] text: Option<String>,
	#[case] reason: &str,
) {
	let mut scope = Scope {
		count,
		text,
		..Default::default()
	};
	let error = put(&mut scope, Uuid::from_u128(1), input())
		.await
		.unwrap_err();
	assert_eq!(error.to_string(), reason);
	assert!(!scope.calls.contains(&"put"));
}
#[rstest]
#[case::deleted("deleted semantic keys cannot be reused", true, false, 0)]
#[case::origin("semantic source identity is immutable", false, true, 0)]
#[case::revision("semantic source revision changed", false, false, 7)]
#[tokio::test]
async fn persisted_identity_and_revision_are_immutable(
	#[case] reason: &str,
	#[case] deleted: bool,
	#[case] origin: bool,
	#[case] revision: i64,
) {
	let mut old = entry();
	old.deleted = deleted;
	if origin {
		old.source = json!({"kind":"artifact","id":Uuid::from_u128(8)});
	}
	let mut scope = Scope {
		old: Some(old),
		..Default::default()
	};
	let mut input = input();
	input.expected_revision = revision;
	if revision > 0 {
		input.metadata = json!({"changed":true});
	}
	let error = put(&mut scope, Uuid::from_u128(1), input)
		.await
		.unwrap_err();
	assert_eq!(error.to_string(), reason);
	assert!(!scope.calls.contains(&"source"));
}
#[rstest]
#[tokio::test]
async fn changed_memory_preserves_creator_and_replaces_point_identity() {
	let old = entry();
	let mut input = input();
	input.expected_revision = old.revision;
	input.source = Source::Memory {
		text: "changed".into(),
	};
	let mut scope = Scope {
		old: Some(old.clone()),
		text: Some("changed".into()),
		..Default::default()
	};
	let result = put(&mut scope, old.workspace_id, input).await.unwrap();
	assert_eq!(result.id, old.id);
	assert_eq!(result.created_by, old.created_by);
	assert_eq!(result.revision, 2);
	assert_ne!(result.point_id, old.point_id);
	assert_eq!(scope.calls.last(), Some(&"history"));
}
#[rstest]
#[case::delete(true)]
#[case::reindex(false)]
#[tokio::test]
async fn managed_deletion_and_reindex_keep_separate_authority_paths(#[case] delete: bool) {
	let old = entry();
	let mut scope = Scope {
		old: Some(old.clone()),
		managed: true,
		..Default::default()
	};
	let result = change(&mut scope, old.workspace_id, old.id, 1, delete)
		.await
		.unwrap();
	assert_eq!(result.revision, 2);
	assert_eq!(result.deleted, delete);
	assert_ne!(result.point_id, old.point_id);
	assert_eq!(scope.calls.contains(&"source"), !delete);
	assert_eq!(scope.calls.contains(&"memory_write"), delete);
	assert_eq!(scope.calls.contains(&"delete_memory"), delete);
	assert_eq!(scope.calls.last(), Some(&"history"));
}
#[rstest]
#[case::delete(true)]
#[case::reindex(false)]
#[tokio::test]
async fn mutation_replays_check_read_authority_and_skip_new_effects(#[case] delete: bool) {
	let mut old = entry();
	old.revision = 2;
	old.deleted = delete;
	let mut scope = Scope {
		old: Some(old.clone()),
		replay: true,
		..Default::default()
	};
	let result = change(&mut scope, old.workspace_id, old.id, 1, delete)
		.await
		.unwrap();
	assert_eq!(result.point_id, old.point_id);
	assert!(scope.calls.contains(&"read"));
	assert!(!scope.calls.contains(&"change"));
	assert!(!scope.calls.contains(&"source"));
}
#[rstest]
#[tokio::test]
async fn managed_memory_denial_prevents_memory_and_semantic_deletion() {
	let old = entry();
	let mut scope = Scope {
		old: Some(old.clone()),
		managed: true,
		fail: Some("memory_write"),
		..Default::default()
	};
	assert_eq!(
		change(&mut scope, old.workspace_id, old.id, 1, true)
			.await
			.unwrap_err()
			.to_string(),
		"memory_write"
	);
	assert!(!scope.calls.contains(&"delete_memory"));
	assert!(!scope.calls.contains(&"change"));
}
#[rstest]
#[tokio::test]
async fn listing_omits_sources_that_have_disappeared() {
	let mut scope = Scope {
		rows: vec![entry()],
		text: None,
		..Default::default()
	};
	assert!(
		entries(&mut scope, Uuid::from_u128(1))
			.await
			.unwrap()
			.is_empty()
	);
	assert_eq!(
		scope.calls,
		["workspace", "index_share", "list", "read", "source"]
	);
}
#[rstest]
#[tokio::test]
async fn history_advances_past_two_hundred_denied_candidates() {
	let row = |sequence, entry_id| History {
		sequence,
		workspace_id: Uuid::from_u128(1),
		entry_id,
		revision: 1,
		state: "PENDING".into(),
		detail: "fixture".into(),
		created_at: Utc::now(),
	};
	let mut scope = Scope {
		deny_entries: true,
		pages: VecDeque::from([
			(201..401)
				.rev()
				.map(|n| row(n, Some(Uuid::from_u128(2))))
				.collect(),
			(1..201).rev().map(|n| row(n, None)).collect(),
		]),
		..Default::default()
	};
	let visible = history(&mut scope, Uuid::from_u128(1)).await.unwrap();
	assert_eq!(
		visible.iter().map(|row| row.sequence).collect::<Vec<_>>(),
		(1..201).rev().collect::<Vec<_>>()
	);
	assert_eq!(scope.cursors, [i64::MAX, 201]);
	assert_eq!(
		scope
			.calls
			.iter()
			.filter(|name| **name == "history_entry")
			.count(),
		200
	);
}

mod configuration;
