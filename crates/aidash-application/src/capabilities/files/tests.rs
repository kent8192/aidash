use super::*;
use crate::capabilities::pod::tests::{Runtime, context};
use crate::ports::capabilities::files::Limits;
use aidash_domain::{policy::Resource, registry::Entry};
use std::sync::Arc;
async fn invoke(
	scope: &mut dyn FileScopePort,
	run: &RunMetadata,
	name: &str,
	input: Value,
	key: &str,
) -> Result<Envelope> {
	match super::invoke(scope, run, name, input, key).await? {
		Invocation::Complete(result) => Ok(result),
		Invocation::Pod(_) => panic!("expected a Working Area result"),
	}
}

struct Scope {
	bytes: Vec<u8>,
	/// What a file holds when it is not the default `bytes`, by path.
	contents: std::collections::BTreeMap<String, Vec<u8>>,
	opens: usize,
	/// Reference records revoked since the Area was given their files.
	revoked: BTreeSet<Uuid>,
	/// Calls that asked whether Reference records are still usable.
	reference_queries: usize,
	search_seconds: u64,
	open_delay: std::time::Duration,
	previous: Value,
	previous_generation: Option<i64>,
	superseded: Vec<Uuid>,
	persisted: Option<Area>,
	events: Vec<(String, Value)>,
	image_publications: Vec<Vec<aidash_domain::semantic::indexing::ImageChange>>,
	snapshot: Option<aidash_domain::registry::bindings::BindingSnapshot>,
	area: Option<Area>,
	deny_entry: Option<String>,
	deny_pinned: Option<String>,
	changed: Option<String>,
	/// Inputs of Completion Gate Checks admitted to the operation journal.
	verified: Vec<Value>,
	/// `(kind, cancel, input)` of every operation control request.
	polled: Vec<(String, bool, Value)>,
	control_result: Option<Value>,
	active_run: Option<Uuid>,
	pending: Vec<u8>,
	cache_entry: Option<(Uuid, String, Value)>,
	references: Vec<FileEntry>,
	pod: Option<PodContext>,
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
			patch_bytes: 64000,
			read_bytes: 16,
			search_matches: 20,
			search_bytes: 10_000,
			search_seconds: self.search_seconds,
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
				saved.identity.registry_deployment == self.local_node()
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
	async fn pod(&mut self, _: &RunMetadata) -> Result<Option<PodContext>> {
		Ok(self.pod.clone())
	}
	async fn bound_references(&mut self, _: &AgentConfig) -> Result<Vec<FileEntry>> {
		Ok(self.references.clone())
	}
	async fn authorize(&mut self, _area: &Area, action: &str) -> Result<()> {
		assert!(matches!(action, "file.read" | "file.write"));
		Ok(())
	}
	async fn current_run(&mut self, _area: &Area) -> Result<Option<Uuid>> {
		Ok(self.active_run)
	}
	async fn require_current(&mut self, _area: &Area, _run: &RunMetadata) -> Result<()> {
		assert!(self.snapshot.is_some());
		Ok(())
	}
	async fn execute(
		&mut self,
		action: Action<'_>,
		_area: &mut Area,
		input: Value,
		_key: &str,
	) -> Result<Value> {
		match action {
			Action::Verify => {
				self.verified.push(input);
				Ok(json!({"operation_id": Uuid::nil(), "status": "prepared"}))
			}
			Action::Control { kind, cancel } => {
				self.polled.push((kind.to_owned(), cancel, input));
				Ok(self
					.control_result
					.clone()
					.unwrap_or(json!({"status": "running"})))
			}
			_ => panic!("unexpected execute effect"),
		}
	}
	async fn output(&mut self, _area: &Area, _id: Uuid) -> Result<Option<(String, i64, String)>> {
		panic!("unexpected output effect")
	}
	async fn open(&mut self, file: &FileEntry) -> Result<Box<dyn VerifiedFile>> {
		self.opens += 1;
		tokio::time::sleep(self.open_delay).await;
		Ok(Box::new(MemoryReader {
			bytes: self.contents.get(&file.path).unwrap_or(&self.bytes).clone(),
			offset: 0,
		}))
	}
	async fn unusable_references(&mut self, ids: &[Uuid]) -> Result<BTreeSet<Uuid>> {
		self.reference_queries += 1;
		Ok(ids
			.iter()
			.copied()
			.filter(|id| self.revoked.contains(id))
			.collect())
	}
	async fn cached(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>> {
		Ok(self
			.cache_entry
			.as_ref()
			.filter(|(k, d, _)| *k == key && d == digest)
			.map(|(_, _, v)| v.clone()))
	}
	async fn cache(&mut self, key: Uuid, digest: &str, result: &Value) -> Result<()> {
		self.cache_entry = Some((key, digest.to_owned(), result.clone()));
		Ok(())
	}
	async fn begin_pending(&mut self, _area: Uuid, _size: u64) -> Result<()> {
		self.pending.clear();
		Ok(())
	}
	async fn read_chunk(&mut self, _file: &FileEntry, _offset: u64) -> Result<Vec<u8>> {
		panic!("unexpected read_chunk effect")
	}
	async fn write_pending(&mut self, bytes: &[u8]) -> Result<()> {
		self.pending.extend_from_slice(bytes);
		Ok(())
	}
	async fn finish_pending(&mut self, expected: &str) -> Result<(Uuid, String)> {
		Ok((Uuid::new_v4(), expected.to_owned()))
	}
	async fn message(&mut self, _thread: Uuid, _id: Uuid) -> Result<Value> {
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
	async fn previous_generation(&mut self, area: &Area) -> Result<i64> {
		Ok(self.previous_generation.unwrap_or(area.generation))
	}
	async fn supersede(&mut self, _area: &Area, file: Uuid) -> Result<()> {
		self.superseded.push(file);
		Ok(())
	}
	async fn persist_manifest(&mut self, area: &Area) -> Result<()> {
		self.persisted = Some(area.clone());
		Ok(())
	}
	async fn event(&mut self, _thread: Uuid, kind: &str, data: Value) -> Result<()> {
		self.events.push((kind.to_owned(), data));
		Ok(())
	}
	async fn image_publication(
		&mut self,
		_: &Area,
		changes: &[aidash_domain::semantic::indexing::ImageChange],
	) -> Result<()> {
		self.image_publications.push(changes.to_vec());
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
			contents: Default::default(),
			opens: 0,
			revoked: BTreeSet::new(),
			reference_queries: 0,
			search_seconds: 5,
			open_delay: std::time::Duration::ZERO,
			previous: area.manifest.clone(),
			previous_generation: None,
			superseded: vec![],
			persisted: None,
			events: vec![],
			image_publications: vec![],
			snapshot: None,
			area: None,
			deny_entry: None,
			deny_pinned: None,
			changed: None,
			verified: vec![],
			polled: vec![],
			control_result: None,
			active_run: None,
			pending: vec![],
			cache_entry: None,
			references: Vec::new(),
			pod: None,
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
fn three_files(area: &mut Area) {
	let file = files(area).unwrap().remove(0);
	area.manifest = json!(["a.md", "b.md", "c.md"].map(|path| FileEntry {
		file_id: Uuid::new_v4(),
		path: path.into(),
		..file.clone()
	}));
}
#[tokio::test]
async fn an_empty_query_lists_every_path_only_in_path_mode() {
	let (mut scope, mut area, _) = fixture(b"needle");
	three_files(&mut area);
	let input = FileSearch {
		query: String::new(),
		mode: SearchMode::Path,
		scope: FileScope::Working,
		path: None,
		cursor: None,
		limit: None,
	};
	let listed = search(&mut scope, &area, input.clone()).await.unwrap();
	assert_eq!(listed["matches"].as_array().unwrap().len(), 3, "{listed}");
	assert_eq!(listed["matches"][2]["path"], "c.md");
	assert_eq!(scope.opens, 0);
	let refused = search(
		&mut scope,
		&area,
		FileSearch {
			mode: SearchMode::Literal,
			..input
		},
	)
	.await
	.unwrap_err();
	assert_eq!(refused.to_string(), "INVALID_SEARCH_BUDGET");
}
#[tokio::test]
async fn a_search_out_of_time_returns_what_it_found_and_how_to_continue() {
	let (mut scope, mut area, _) = fixture(b"needle");
	three_files(&mut area);
	scope.search_seconds = 1;
	scope.open_delay = std::time::Duration::from_millis(500);
	let input = FileSearch {
		query: "needle".into(),
		mode: SearchMode::Literal,
		scope: FileScope::Working,
		path: None,
		cursor: None,
		limit: None,
	};
	let first = search(&mut scope, &area, input.clone()).await.unwrap();
	assert_eq!(first["truncated"], true, "{first}");
	let found = first["matches"].as_array().unwrap().len();
	assert!((1..3).contains(&found), "{first}");
	let second = search(
		&mut scope,
		&area,
		FileSearch {
			cursor: first["next_cursor"].as_str().map(str::to_owned),
			..input
		},
	)
	.await
	.unwrap();
	let rest = second["matches"].as_array().unwrap();
	assert!(!rest.is_empty(), "{second}");
	assert_eq!(
		rest.last().unwrap()["path"],
		if second["truncated"] == true {
			"b.md"
		} else {
			"c.md"
		}
	);
}
#[tokio::test]
async fn one_very_long_line_does_not_stop_a_search_of_everything_else() {
	let (mut scope, area, _) = fixture(&vec![b'x'; 70_000]);
	let input = FileSearch {
		query: "needle".into(),
		mode: SearchMode::Literal,
		scope: FileScope::Working,
		path: None,
		cursor: None,
		limit: None,
	};
	let found = search(&mut scope, &area, input.clone()).await.unwrap();
	assert_eq!(found["unavailable"][0]["error"], "SEARCH_LINE_LIMIT");
	assert_eq!(found["truncated"], false);
	let selected = search(
		&mut scope,
		&area,
		FileSearch {
			path: Some("notes.txt".into()),
			..input
		},
	)
	.await
	.unwrap_err();
	assert!(selected.to_string().contains("SEARCH_LINE_LIMIT"));
}
#[tokio::test]
async fn a_full_page_is_returned_with_a_cursor_when_the_next_file_has_a_very_long_line() {
	let (mut scope, mut area, _) = fixture(b"needle\n");
	three_files(&mut area);
	scope.contents.insert("b.md".into(), vec![b'x'; 70_000]);
	let input = FileSearch {
		query: "needle".into(),
		mode: SearchMode::Literal,
		scope: FileScope::Working,
		path: None,
		cursor: None,
		limit: Some(1),
	};
	let first = search(&mut scope, &area, input.clone()).await.unwrap();
	assert_eq!(first["matches"][0]["path"], "a.md", "{first}");
	assert_eq!(first["truncated"], true, "{first}");
	let mut found = vec!["a.md".to_owned()];
	let mut unavailable = 0;
	let mut cursor = first["next_cursor"].as_str().map(str::to_owned);
	while cursor.is_some() {
		let page = search(
			&mut scope,
			&area,
			FileSearch {
				cursor: cursor.take(),
				..input.clone()
			},
		)
		.await
		.unwrap();
		found.extend(
			page["matches"]
				.as_array()
				.unwrap()
				.iter()
				.map(|item| item["path"].as_str().unwrap().to_owned()),
		);
		unavailable += page["unavailable"].as_array().unwrap().len();
		cursor = page["next_cursor"].as_str().map(str::to_owned);
	}
	assert_eq!(found, ["a.md", "c.md"]);
	assert_eq!(unavailable, 1, "the long line is reported once");
}
/// A Reference file of the Area's set, taken from Reference record `record`.
fn reference_file(path: &str, record: Uuid) -> FileEntry {
	FileEntry {
		file_id: Uuid::new_v4(),
		path: format!("references/{path}"),
		digest: "digest".into(),
		size: 7,
		media_type: "text/plain".into(),
		scope: FileScope::References,
		provenance: json!({"kind":"reference","id":record}),
	}
}
#[tokio::test]
async fn a_revoked_reference_file_is_neither_read_searched_nor_copied() {
	let (mut scope, run, _) = invoke_fixture("file_read", json!({}));
	scope.active_run = Some(run.id);
	let (kept, gone) = (Uuid::new_v4(), Uuid::new_v4());
	let (kept_file, gone_file) = (
		reference_file("kept.md", kept),
		reference_file("gone.md", gone),
	);
	for file in [&kept_file, &gone_file] {
		scope
			.contents
			.insert(file.path.clone(), b"needle\n".to_vec());
	}
	let mut area = scope.area.clone().unwrap();
	area.manifest = json!([kept_file, gone_file]);
	scope.area = Some(area.clone());
	scope.revoked.insert(gone);

	let read_text = |file: &FileEntry| selection(file, Representation::Text, None, None);
	assert_eq!(
		read(&mut scope, &area, read_text(&kept_file))
			.await
			.unwrap()["content"],
		"needle\n"
	);
	for representation in [Representation::Text, Representation::Metadata] {
		let refused = read(
			&mut scope,
			&area,
			selection(&gone_file, representation, None, None),
		)
		.await
		.unwrap_err();
		assert!(
			matches!(&refused, Error::NotFound(message) if message.contains("gone.md")),
			"{refused}"
		);
	}

	let paths = |found: &Value| {
		found["matches"]
			.as_array()
			.unwrap()
			.iter()
			.map(|item| item["path"].as_str().unwrap().to_owned())
			.collect::<Vec<_>>()
	};
	let input = FileSearch {
		query: "needle".into(),
		mode: SearchMode::Literal,
		scope: FileScope::References,
		path: None,
		cursor: None,
		limit: None,
	};
	scope.reference_queries = 0;
	let found = search(&mut scope, &area, input.clone()).await.unwrap();
	assert_eq!(paths(&found), ["references/kept.md"], "{found}");
	assert_eq!(scope.reference_queries, 1, "one query for the whole set");
	let listed = search(
		&mut scope,
		&area,
		FileSearch {
			query: String::new(),
			mode: SearchMode::Path,
			..input
		},
	)
	.await
	.unwrap();
	assert_eq!(paths(&listed), ["references/kept.md"], "{listed}");

	let refused = materialize(
		&mut scope,
		&run,
		Materialize {
			idempotency_key: Uuid::new_v4(),
			expected_revision: area.revision,
			path: "copy.md".into(),
			source: MaterializeSource::File {
				file_id: gone_file.file_id,
				expected_digest: gone_file.digest.clone(),
			},
		},
	)
	.await
	.unwrap_err();
	assert!(matches!(refused, Error::NotFound(_)), "{refused}");
	assert!(scope.pending.is_empty(), "nothing was copied");
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

#[tokio::test]
async fn a_receiving_area_publishes_without_a_thread_index_or_event() {
	let (mut scope, mut area, mut image) = fixture(b"bytes");
	image.path = "image.png".into();
	image.media_type = "image/png".into();
	scope.previous = json!([]);
	area.manifest = json!([image]);
	area.constraints = json!([
		aidash_domain::capabilities::sessions::remote_home_constraint("aidash://home/node")
	]);
	publish(&mut scope, &mut area).await.unwrap();
	// The revision is still saved, so the Area's readers see the Reference set.
	assert_eq!(scope.persisted.as_ref().unwrap().revision, area.revision);
	assert!(scope.image_publications.is_empty());
	assert!(scope.events.is_empty());
}

#[tokio::test]
async fn publication_records_image_versions_in_publication_order() {
	use aidash_domain::semantic::indexing::ImageChange;
	let (mut scope, mut area, mut old) = fixture(b"bytes");
	old.path = "image.png".into();
	old.media_type = "image/png".into();
	let mut replacement = old.clone();
	replacement.file_id = Uuid::new_v4();
	let mut added = old.clone();
	added.file_id = Uuid::new_v4();
	added.path = "next.png".into();
	scope.previous = json!([old]);
	area.manifest = json!([replacement, added]);
	publish(&mut scope, &mut area).await.unwrap();
	assert_eq!(scope.image_publications.len(), 1);
	let changes = &scope.image_publications[0];
	assert!(matches!(&changes[0], ImageChange::Removed { file_id, .. } if *file_id == old.file_id));
	assert!(
		matches!(&changes[1], ImageChange::Published { file_id, .. } if *file_id == replacement.file_id)
	);
	assert!(
		matches!(&changes[2], ImageChange::Published { file_id, .. } if *file_id == added.file_id)
	);
}

#[tokio::test]
async fn a_new_area_generation_republishes_unchanged_image_versions() {
	use aidash_domain::semantic::indexing::ImageChange;
	let (mut scope, mut area, mut image) = fixture(b"bytes");
	image.path = "image.png".into();
	image.media_type = "image/png".into();
	scope.previous = json!([image]);
	scope.previous_generation = Some(area.generation - 1);
	area.manifest = json!([image]);
	publish(&mut scope, &mut area).await.unwrap();
	let changes = &scope.image_publications[0];
	assert!(
		matches!(&changes[..], [ImageChange::Removed { file_id: old, .. }, ImageChange::Published { file_id: new, .. }] if *old == image.file_id && *new == image.file_id)
	);
}

#[tokio::test]
async fn publication_detects_images_in_generic_binary_files_by_signature() {
	let (mut scope, mut area, mut file) = fixture(b"\x89PNG\r\n\x1a\nimage");
	file.path = "without-extension".into();
	file.media_type = "application/octet-stream".into();
	scope.previous = json!([]);
	area.manifest = json!([file]);
	publish(&mut scope, &mut area).await.unwrap();
	assert_eq!(area.manifest[0]["media_type"], "image/png");
	assert_eq!(scope.image_publications.len(), 1);
}

fn invoke_fixture(operation: &str, narrow: Value) -> (Scope, RunMetadata, FileEntry) {
	let (mut scope, area, mut file) = fixture(b"abcdefgh");
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
		node: "brave-otter-k3m9q2xa".parse().unwrap(),
		id: Uuid::new_v4(),
		task_id: Uuid::new_v4(),
		thread_id: area.thread_id,
		home_node: node.into(),
		agent_id: agent.id,
		agent_version: agent.version,
		phase: aidash_domain::RunPhase::Ready,
		control: aidash_domain::RunControl::Active,
		step: 0,
		revision: 1,
		observed_input_seq: 0,
		ledger_worker_ready: true,
		control_revision: 0,
		control_source: Default::default(),
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: chrono::Utc::now(),
	};
	file.scope = FileScope::References;
	file.provenance = json!({"kind": "reference", "id": Uuid::new_v4()});
	scope.references = vec![file.clone()];
	scope.area = Some(area);
	(scope, run, file)
}

#[tokio::test]
async fn message_files_are_bounded_snapshots_of_scratch_paths() {
	let (mut scope, run, file) = invoke_fixture("file_read", json!({}));
	let runtime = Arc::new(Runtime::default());
	runtime.file(&file.path, b"abcdefgh");
	scope.pod = Some(context(runtime.clone()));

	let files = snapshot_message_files(&mut scope, &run, std::slice::from_ref(&file.path))
		.await
		.unwrap();
	assert_eq!(files[0].0.path, file.path);
	assert_eq!(files[0].1, b"abcdefgh");
	assert_eq!(runtime.calls(), ["file_manifest", "file_read"]);
	assert_eq!(scope.opens, 0);

	assert!(matches!(
		snapshot_message_files(&mut scope, &run, &["missing.png".into()]).await,
		Err(Error::NotFound(_))
	));
	assert!(matches!(
		snapshot_message_files(&mut scope, &run, &vec![file.path; 9]).await,
		Err(Error::Invalid(_))
	));
	runtime.file("large.txt", &vec![0; 8 * 1024 * 1024 + 1]);
	assert!(matches!(
		snapshot_message_files(&mut scope, &run, &["large.txt".into()]).await,
		Err(Error::Invalid(message)) if message.contains("byte limit")
	));
}

#[tokio::test]
async fn committed_message_replay_never_reopens_a_changed_or_deleted_file() {
	let (mut scope, run, file) = invoke_fixture("file_read", json!({}));
	let runtime = Arc::new(Runtime::default());
	runtime.file(&file.path, b"abcdefgh");
	scope.pod = Some(context(runtime.clone()));
	let key = Uuid::new_v4();
	let digest = "original request";
	let paths = vec![file.path];
	let first = snapshot_message_files_once(&mut scope, &run, key, digest, false, &paths)
		.await
		.unwrap()
		.unwrap();
	assert_eq!(first[0].1, b"abcdefgh");
	scope
		.cache(key, digest, &json!({"sent":true}))
		.await
		.unwrap();

	runtime.file(&paths[0], b"changed");
	assert!(
		snapshot_message_files_once(&mut scope, &run, key, digest, false, &paths)
			.await
			.unwrap()
			.is_none()
	);
	scope.cache_entry = None;
	assert!(
		snapshot_message_files_once(&mut scope, &run, key, digest, true, &paths)
			.await
			.unwrap()
			.is_none()
	);
	assert_eq!(runtime.calls(), ["file_manifest", "file_read"]);
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
#[case::read("file_read")]
#[case::search("file_search")]
#[tokio::test]
async fn area_state_does_not_gate_reference_reads(#[case] operation: &str) {
	let (mut scope, run, file) = invoke_fixture(operation, json!({}));
	let input = if operation == "file_read" {
		json!({"file_id":file.file_id,"representation":"text","expected_digest":file.digest})
	} else {
		json!({"query":"abc","mode":"literal","scope":"references"})
	};
	for state in ["running", "uncertain"] {
		scope.area.as_mut().unwrap().state = state.into();
		let result = invoke(&mut scope, &run, operation, input.clone(), "request")
			.await
			.unwrap();
		assert_eq!(result.status, "completed");
	}
	assert_eq!(scope.opens, 2);
}

#[tokio::test]
async fn reference_reads_reject_old_area_selectors() {
	let (mut scope, run, file) = invoke_fixture("file_read", json!({}));
	for field in ["thread_id", "area_id"] {
		let mut input = json!({"file_id":file.file_id,"representation":"text"});
		input[field] = json!(Uuid::new_v4());
		assert!(matches!(
			invoke(&mut scope, &run, "file_read", input, "request").await,
			Err(Error::Invalid(_))
		));
	}
	assert_eq!(scope.opens, 0);
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

#[tokio::test]
async fn a_completion_check_is_admitted_like_the_models_own_shell_call() {
	let (mut scope, run, _) = invoke_fixture("shell", json!({}));
	let input = json!({"idempotency_key":Uuid::nil(),"command":"cargo test","expected_revision":4});
	verify(&mut scope, &run, input.clone(), "check")
		.await
		.unwrap();
	assert_eq!(scope.verified, [input]);
}

#[rstest::rstest]
#[case("catalog")]
#[case("installation")]
#[case("digest")]
#[tokio::test]
async fn a_completion_check_rechecks_the_bound_shell_before_any_effect(#[case] withdrawal: &str) {
	let (mut scope, run, _) = invoke_fixture("shell", json!({}));
	let id = scope
		.binding_snapshot()
		.unwrap()
		.operation("shell")
		.unwrap()
		.identity
		.id
		.clone();
	match withdrawal {
		"catalog" => scope.deny_entry = Some(id),
		"installation" => scope.deny_pinned = Some(id),
		_ => scope.changed = Some(id),
	}
	let result = verify(&mut scope, &run, json!({"command":"cargo test"}), "check").await;
	if withdrawal == "digest" {
		assert!(matches!(result, Err(Error::Conflict(_))));
	} else {
		assert!(matches!(result, Err(Error::Forbidden)));
	}
	assert!(scope.verified.is_empty());
}

/// Polling a Check reads its operation as a `verification`, never as the
/// model's shell, and never cancels it.
#[tokio::test]
async fn a_completion_check_is_polled_as_a_verification_operation() {
	let (mut scope, run, _) = invoke_fixture("shell", json!({}));
	let operation = Uuid::from_u128(7);
	let result = verification(&mut scope, &run, operation, "poll")
		.await
		.unwrap();
	assert_eq!(result["status"], "running");
	assert_eq!(
		scope.polled,
		[(
			aidash_domain::capabilities::operations::VERIFICATION.to_owned(),
			false,
			json!({"operation_id": operation}),
		)]
	);
	assert!(scope.verified.is_empty());
}

#[tokio::test]
async fn polling_preserves_the_runner_error_message_in_the_tool_result() {
	let (mut scope, run, _) = invoke_fixture("shell_poll", json!({}));
	scope.area.as_mut().unwrap().state = "running".into();
	scope.control_result = Some(json!({"status":"uncertain","error":{
		"code":"OUTPUT_LOST","message":"File changes were not saved."}}));
	let response = invoke(
		&mut scope,
		&run,
		"shell_poll",
		json!({"operation_id":Uuid::new_v4()}),
		"poll",
	)
	.await
	.unwrap();
	assert_eq!(response.status, "uncertain");
	assert_eq!(
		response.result["error"]["message"],
		"File changes were not saved."
	);
}

#[tokio::test]
async fn a_completion_check_needs_a_bound_shell() {
	let (mut scope, run, _) = invoke_fixture("file_read", json!({}));
	assert!(matches!(
		verify(&mut scope, &run, json!({"command":"cargo test"}), "check").await,
		Err(Error::Domain(aidash_domain::Error::Invalid(message))) if message.contains("shell")
	));
	assert!(scope.verified.is_empty());
}

#[test]
fn a_working_area_read_in_the_new_schema_reaches_the_strict_legacy_shape() {
	let file = Uuid::new_v4();
	let read = legacy_input(
		"file_read",
		json!({"file_id": file, "path": null, "area_id": null}),
	);
	let parsed: aidash_domain::capabilities::files::FileRead =
		serde_json::from_value(read).unwrap();
	assert_eq!(parsed.file_id, file);
	assert!(matches!(
		parsed.representation,
		aidash_domain::capabilities::files::Representation::Text
	));
	let search = legacy_input(
		"file_search",
		json!({"query": "x", "mode": "literal", "scope": "working", "glob": null}),
	);
	serde_json::from_value::<aidash_domain::capabilities::files::FileSearch>(search).unwrap();
}
