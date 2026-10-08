use super::*;
use crate::ports::capabilities::files::Limits;
use aidash_domain::{policy::Resource, registry::Entry};
struct Scope {
	bytes: Vec<u8>,
	opens: usize,
	previous: Value,
	superseded: Vec<Uuid>,
	persisted: Option<Area>,
	events: Vec<(String, Value)>,
	snapshot: Option<aidash_domain::registry::bindings::BindingSnapshot>,
	area: Option<Area>,
	deny_entry: Option<String>,
	deny_pinned: Option<String>,
	changed: Option<String>,
}
#[async_trait::async_trait]
impl FileScopePort for Scope {
	fn local_node(&self) -> &str {
		"aidash://local"
	}
	fn binding_snapshot(&self) -> Result<&aidash_domain::registry::bindings::BindingSnapshot> {
		self.snapshot.as_ref().ok_or(Error::Forbidden)
	}
	fn limits(&self) -> Result<Limits> {
		Ok(Limits {
			working_bytes: 1_000_000,
			read_bytes: 16,
			search_matches: 20,
			search_bytes: 10_000,
			search_seconds: 5,
		})
	}
	fn policy_revision(&self) -> i64 {
		0
	}
	fn resource(&self, _kind: &str, _id: &str, _attributes: Value) -> Resource {
		assert!(self.snapshot.is_some());
		Resource {
			tenant: "tenant".into(),
			kind: _kind.into(),
			id: _id.into(),
			attributes: _attributes,
		}
	}
	async fn entry(&mut self, reference: &EntityRef, _: &str) -> Result<Entry> {
		if self.deny_entry.as_deref() == Some(reference.id.as_str()) {
			return Err(Error::Forbidden);
		}
		let mut current = self
			.binding_snapshot()?
			.definitions
			.iter()
			.find(|saved| {
				saved.identity.registry_node == self.local_node()
					&& saved.identity.local() == *reference
			})
			.unwrap()
			.definition
			.clone();
		if self.changed.as_deref() == Some(reference.id.as_str()) {
			current.tags.push("changed".into());
		}
		Ok(current)
	}
	async fn check_pinned(&mut self, entry: &Entry) -> Result<()> {
		if self.deny_pinned.as_deref() == Some(entry.id.as_str()) {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn effective(&mut self, _reference: &EntityRef) -> Result<Entry> {
		panic!("unexpected effective effect")
	}
	async fn require(&mut self, _resource: &Resource, _action: &str) -> Result<()> {
		assert!(self.snapshot.is_some());
		Ok(())
	}
	async fn serialize_sharing(&mut self) -> Result<()> {
		panic!("unexpected serialize_sharing effect")
	}
	async fn for_run(&mut self, _run: &RunMetadata) -> Result<Area> {
		Ok(self.area.as_ref().expect("admitted area").clone())
	}
	async fn authorize(&mut self, _area: &Area, _action: &str) -> Result<()> {
		panic!("unexpected authorize effect")
	}
	async fn current_run(&mut self, _area: &Area) -> Result<Option<Uuid>> {
		panic!("unexpected current_run effect")
	}
	async fn require_current(&mut self, _area: &Area, _run: &RunMetadata) -> Result<()> {
		assert!(self.snapshot.is_some());
		Ok(())
	}
	async fn execute(
		&mut self,
		_action: Action<'_>,
		_area: &mut Area,
		_input: Value,
		_key: &str,
	) -> Result<Value> {
		panic!("unexpected execute effect")
	}
	async fn output(&mut self, _area: &Area, _id: Uuid) -> Result<Option<(String, i64, String)>> {
		panic!("unexpected output effect")
	}
	async fn open(&mut self, _file: &FileEntry) -> Result<Box<dyn VerifiedFile>> {
		self.opens += 1;
		Ok(Box::new(MemoryReader {
			bytes: self.bytes.clone(),
			offset: 0,
		}))
	}
	async fn cached(&mut self, _key: Uuid, _digest: &str) -> Result<Option<Value>> {
		panic!("unexpected cached effect")
	}
	async fn cache(&mut self, _key: Uuid, _digest: &str, _result: &Value) -> Result<()> {
		panic!("unexpected cache effect")
	}
	async fn begin_pending(&mut self, _area: Uuid, _size: u64) -> Result<()> {
		panic!("unexpected begin_pending effect")
	}
	async fn read_chunk(&mut self, _file: &FileEntry, _offset: u64) -> Result<Vec<u8>> {
		panic!("unexpected read_chunk effect")
	}
	async fn write_pending(&mut self, _bytes: &[u8]) -> Result<()> {
		panic!("unexpected write_pending effect")
	}
	async fn finish_pending(&mut self, _expected: &str) -> Result<(Uuid, String)> {
		panic!("unexpected finish_pending effect")
	}
	async fn message(&mut self, _workspace: Uuid, _id: Uuid) -> Result<Value> {
		panic!("unexpected message effect")
	}
	async fn documents(&mut self, _entry: &Entry) -> Result<Value> {
		panic!("unexpected documents effect")
	}
	async fn text_file(
		&mut self,
		_area: Uuid,
		_path: String,
		_text: &str,
		_scope: FileScope,
		_provenance: Value,
	) -> Result<FileEntry> {
		panic!("unexpected text_file effect")
	}
	async fn previous_manifest(&mut self, _area: &Area) -> Result<Value> {
		Ok(self.previous.clone())
	}
	async fn supersede(&mut self, _area: &Area, file: Uuid) -> Result<()> {
		self.superseded.push(file);
		Ok(())
	}
	async fn persist_manifest(&mut self, area: &Area) -> Result<()> {
		self.persisted = Some(area.clone());
		Ok(())
	}
	async fn event(&mut self, _workspace: Uuid, kind: &str, data: Value) -> Result<()> {
		self.events.push((kind.to_owned(), data));
		Ok(())
	}
}
fn fixture(bytes: &[u8]) -> (Scope, Area, FileEntry) {
	let file = FileEntry {
		file_id: Uuid::new_v4(),
		path: "notes.txt".into(),
		digest: "digest".into(),
		size: bytes.len() as u64,
		media_type: "text/plain".into(),
		scope: FileScope::Working,
		provenance: json!({}),
	};
	let area = Area {
		id: Uuid::new_v4(),
		tenant: "tenant".into(),
		home_node: "home".into(),
		workspace_id: Uuid::new_v4(),
		thread_id: Uuid::new_v4(),
		agent_id: "agent".into(),
		owner: "alice".into(),
		generation: 7,
		revision: 4,
		epoch: 2,
		state: "active".into(),
		manifest: json!([file]),
		constraints: json!([]),
		next_sequence: 1,
	};
	(
		Scope {
			bytes: bytes.to_vec(),
			opens: 0,
			previous: area.manifest.clone(),
			superseded: vec![],
			persisted: None,
			events: vec![],
			snapshot: None,
			area: None,
			deny_entry: None,
			deny_pinned: None,
			changed: None,
		},
		area,
		file,
	)
}
fn selection(
	file: &FileEntry,
	representation: Representation,
	offset: Option<usize>,
	max_bytes: Option<usize>,
) -> FileRead {
	FileRead {
		file_id: file.file_id,
		representation,
		offset,
		max_bytes,
		expected_digest: Some(file.digest.clone()),
	}
}
#[tokio::test]
async fn byte_pages_never_split_a_utf8_character() {
	let (mut scope, area, file) = fixture("a界b".as_bytes());
	let result = read(
		&mut scope,
		&area,
		selection(&file, Representation::Text, None, Some(3)),
	)
	.await
	.unwrap();
	assert_eq!(result["content"], "a");
	assert_eq!(result["next_offset"], 1);
	assert_eq!(result["truncated"], true);
	assert_eq!(scope.opens, 1);
	let result = read(
		&mut scope,
		&area,
		selection(&file, Representation::Text, Some(1), Some(4)),
	)
	.await
	.unwrap();
	assert_eq!(result["content"], "界b");
	assert_eq!(result["next_offset"], Value::Null);
	assert_eq!(result["truncated"], false);
}
#[tokio::test]
async fn a_page_too_small_for_the_next_character_fails_without_a_stalled_cursor() {
	let (mut scope, area, file) = fixture("界".as_bytes());
	let error = read(
		&mut scope,
		&area,
		selection(&file, Representation::Text, None, Some(2)),
	)
	.await
	.unwrap_err();
	assert_eq!(
		error.to_string(),
		"READ_BUDGET: max_bytes cannot contain the next UTF-8 character"
	);
}
#[tokio::test]
async fn an_offset_inside_a_character_cannot_disclose_corrupted_text() {
	let (mut scope, area, file) = fixture("界".as_bytes());
	let error = read(
		&mut scope,
		&area,
		selection(&file, Representation::Text, Some(1), Some(2)),
	)
	.await
	.unwrap_err();
	assert_eq!(
		error.to_string(),
		"REPRESENTATION_UNAVAILABLE_OR_INVALID_UTF8_OFFSET"
	);
}
#[tokio::test]
async fn metadata_does_not_open_file_contents() {
	let (mut scope, area, file) = fixture(b"private");
	let result = read(
		&mut scope,
		&area,
		selection(&file, Representation::Metadata, None, None),
	)
	.await
	.unwrap();
	assert_eq!(result["metadata"]["file_id"], json!(file.file_id));
	assert_eq!(result["digest"], "digest");
	assert_eq!(result.get("content"), None);
	assert_eq!(scope.opens, 0);
}
#[tokio::test]
async fn a_changed_manifest_invalidates_search_before_any_file_is_opened() {
	let (mut scope, area, _) = fixture(b"needle");
	let hash = aidash_domain::registry::rules::digest(&json!([
		"needle",
		SearchMode::Literal,
		FileScope::Working,
		Option::<String>::None
	]));
	let cursor = Cursor {
		area: area.id,
		generation: area.generation,
		revision: area.revision - 1,
		query: hash,
		file: 0,
		line: 0,
	};
	let input = FileSearch {
		query: "needle".into(),
		mode: SearchMode::Literal,
		scope: FileScope::Working,
		path: None,
		cursor: Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor).unwrap())),
		limit: Some(1),
	};
	let error = search(&mut scope, &area, input).await.unwrap_err();
	assert_eq!(error.to_string(), "STALE_CURSOR");
	assert_eq!(scope.opens, 0);
}
#[tokio::test]
async fn search_reconnection_resumes_at_the_first_undelivered_match() {
	let (mut scope, area, _) = fixture(b"needle one\nneedle two\n");
	let input = FileSearch {
		query: "needle".into(),
		mode: SearchMode::Literal,
		scope: FileScope::Working,
		path: None,
		cursor: None,
		limit: Some(1),
	};
	let first = search(&mut scope, &area, input.clone()).await.unwrap();
	let cursor = first["next_cursor"].as_str().unwrap().to_owned();
	let second = search(
		&mut scope,
		&area,
		FileSearch {
			cursor: Some(cursor),
			..input
		},
	)
	.await
	.unwrap();
	assert_eq!(first["matches"].as_array().unwrap().len(), 1);
	assert_eq!(first["matches"][0]["location"]["line"], 1);
	assert_eq!(first["truncated"], true);
	assert_eq!(second["matches"].as_array().unwrap().len(), 1);
	assert_eq!(second["matches"][0]["location"]["line"], 2);
	assert_eq!(second["next_cursor"], Value::Null);
}
#[tokio::test]
async fn publication_tombstones_only_removed_working_objects() {
	let (mut scope, mut area, file) = fixture(b"bytes");
	let received = FileEntry {
		file_id: Uuid::new_v4(),
		scope: FileScope::Received,
		..file.clone()
	};
	let reference = FileEntry {
		file_id: Uuid::new_v4(),
		scope: FileScope::References,
		..file.clone()
	};
	scope.previous = json!([file, received, reference]);
	area.manifest = json!([]);
	publish(&mut scope, &mut area).await.unwrap();
	assert_eq!(scope.superseded, vec![file.file_id]);
	assert_eq!(area.revision, 5);
	assert_eq!(scope.persisted.as_ref().unwrap().revision, 5);
	assert_eq!(
		scope.events,
		vec![(
			"capability.files_changed".into(),
			json!({"area_id":area.id,"revision":5,"generation":7})
		)]
	);
}

fn invoke_fixture(operation: &str, narrow: Value) -> (Scope, RunMetadata, FileEntry) {
	let (mut scope, area, file) = fixture(b"abcdefgh");
	let node = "aidash://local";
	let mut agent = crate::test_support::agent("agent");
	let mut binding = crate::test_support::binding("tool", node, &format!("aidash.{operation}"));
	binding["narrow"] = narrow;
	agent.config["bindings"] = json!([binding]);
	let tool = crate::test_support::entry(
		&format!("aidash.{operation}"),
		"tool",
		json!(aidash_domain::tool::providers::core_descriptor(node, operation).unwrap()),
	);
	scope.snapshot = Some(crate::test_support::resolve(
		node,
		&agent,
		false,
		vec![tool],
	));
	let run = RunMetadata {
		id: Uuid::new_v4(),
		task_id: Uuid::new_v4(),
		workspace_id: area.workspace_id,
		home_node: node.into(),
		agent_id: agent.id,
		agent_version: agent.version,
		phase: aidash_domain::RunPhase::Ready,
		control: aidash_domain::RunControl::Active,
		step: 0,
		revision: 1,
		observed_input_seq: 0,
		ledger_worker_ready: true,
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: chrono::Utc::now(),
	};
	scope.area = Some(area);
	(scope, run, file)
}
#[rstest::rstest]
#[case("outbound_get", json!({"allowed_hosts":["example.com"]}), json!({"url":"https://outside.example/data"}))]
#[case("file_read", json!({"scope":{"representation":["text"]}}), json!({"representation":"model_input"}))]
#[case("file_read", json!({"limits":{"max_bytes":3}}), json!({"max_bytes":4}))]
#[tokio::test]
async fn direct_invocations_reject_inputs_outside_admitted_binding_before_effects(
	#[case] operation: &str,
	#[case] narrow: Value,
	#[case] input: Value,
) {
	let (mut scope, run, _) = invoke_fixture(operation, narrow);
	scope.area = None;
	assert!(matches!(
		invoke(&mut scope, &run, operation, input, "request").await,
		Err(Error::Domain(aidash_domain::Error::Invalid(_)))
	));
	assert_eq!(scope.opens, 0);
	assert!(scope.events.is_empty());
}
#[tokio::test]
async fn direct_file_read_applies_the_admitted_limit_when_input_omits_it() {
	let (mut scope, run, file) = invoke_fixture("file_read", json!({"limits":{"max_bytes":3}}));
	let result = invoke(
		&mut scope,
		&run,
		"file_read",
		json!({"file_id":file.file_id,"representation":"text","expected_digest":file.digest}),
		"request",
	)
	.await
	.unwrap();
	assert_eq!(result.result["content"], "abc");
	assert_eq!(result.result["next_offset"], 3);
	assert_eq!(scope.opens, 1);
}

#[rstest::rstest]
#[case("shell", "catalog")]
#[case("shell", "installation")]
#[case("shell", "digest")]
#[case("outbound_get", "catalog")]
#[case("outbound_get", "installation")]
#[case("outbound_get", "digest")]
#[tokio::test]
async fn direct_invocations_recheck_the_bound_definition_before_any_effect(
	#[case] operation: &str,
	#[case] withdrawal: &str,
) {
	let (mut scope, run, _) = invoke_fixture(operation, json!({}));
	let id = scope
		.binding_snapshot()
		.unwrap()
		.operation(operation)
		.unwrap()
		.identity
		.id
		.clone();
	match withdrawal {
		"catalog" => scope.deny_entry = Some(id),
		"installation" => scope.deny_pinned = Some(id),
		"digest" => scope.changed = Some(id),
		_ => unreachable!(),
	}
	scope.area = None;
	let result = invoke(&mut scope, &run, operation, json!({}), "request").await;
	if withdrawal == "digest" {
		assert!(matches!(result, Err(Error::Conflict(_))));
	} else {
		assert!(matches!(result, Err(Error::Forbidden)));
	}
	assert_eq!(scope.opens, 0);
	assert!(scope.events.is_empty());
}
