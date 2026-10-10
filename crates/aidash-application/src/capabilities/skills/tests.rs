//! Unit tests for services::skills.
use super::*;
use aidash_domain::capabilities::skills::SkillMetadata;

#[rstest::rstest]
#[case("abc")]
#[case("日本語")]
#[case("A😀B")]
#[case("e\u{301}界")]
fn text_pages_reconstruct_using_scalar_cursors(#[case] text: &str) {
	let mut offset = 0;
	let mut reconstructed = String::new();
	loop {
		let (chunk, next) = text_chunk(text, offset, 1, 4).unwrap();
		assert_eq!(chunk.chars().count(), 1);
		assert!(chunk.len() <= 4);
		reconstructed.push_str(chunk);
		let Some(next) = next else {
			break;
		};
		assert_eq!(next, offset + 1);
		offset = next;
	}
	assert_eq!(reconstructed, text);
}

#[rstest::rstest]
fn scalar_quota_and_encoded_byte_cap_are_independent() {
	assert_eq!(text_chunk("😀界a", 0, 100, 4).unwrap(), ("😀", Some(1)));
	assert_eq!(text_chunk("😀界a", 1, 100, 4).unwrap(), ("界a", None));
	assert!(
		matches!(text_chunk("😀", 0, 1, 3), Err(Error::Invalid(ref code)) if code == "READ_BUDGET")
	);
	assert_eq!(text_chunk("😀", 1, 1, 1).unwrap(), ("", None));
	assert!(
		matches!(text_chunk("😀", 2, 1, 4), Err(Error::Invalid(ref code)) if code == "INVALID_READ_RANGE")
	);
}

#[rstest::rstest]
fn skill_read_schema_accepts_chars_and_rejects_old_byte_input() {
	let request = json!({"skill_id":Uuid::nil(), "digest":"digest", "path":"guide.md", "offset":1, "max_chars":2});
	let parsed: SkillRead = serde_json::from_value(request.clone()).unwrap();
	assert_eq!(parsed.max_chars, Some(2));
	let mut old = request;
	old.as_object_mut().unwrap().remove("max_chars");
	old["max_bytes"] = json!(2);
	assert!(serde_json::from_value::<SkillRead>(old).is_err());
}

#[rstest::rstest]
fn loaded_skill_reserve_matches_the_escaped_system_prompt() {
	let instructions = "---\nname: check\n---\nA quoted \"line\" and a newline\n";
	let metadata = SkillMetadata {
		skill_id: Uuid::new_v4(),
		name: "check".into(),
		description: "Inspect input".into(),
		origin: "test".into(),
		digest: "digest".into(),
		license: None,
	};
	let mut pinned = vec![Pinned {
		loaded: true,
		metadata: metadata.clone(),
		files: vec![FileEntry {
			file_id: Uuid::new_v4(),
			path: "SKILL.md".into(),
			digest: "digest".into(),
			size: instructions.len() as u64,
			media_type: "application/octet-stream".into(),
			scope: FileScope::References,
			provenance: json!({}),
		}],
		instruction_json_len: Some(escaped_instruction_len(instructions).unwrap()),
	}];
	let actual = format!(
		"\nPinned Skills (select by UUID and origin; use skill_load):\n{}\n{}\n",
		serde_json::to_string(&metadata).unwrap(),
		instructions
	);
	assert_eq!(
		pinned_context_reserve(&pinned).unwrap(),
		escaped_instruction_len(&actual).unwrap()
	);
	pinned[0].instruction_json_len = None;
	assert!(pinned_context_reserve(&pinned).unwrap() >= escaped_instruction_len(&actual).unwrap());
}

#[rstest::rstest]
fn unloaded_skills_reserve_only_metadata() {
	let skill = metadata();
	let expected = format!(
		"\nPinned Skills (select by UUID and origin; use skill_load):\n{}\n",
		serde_json::to_string(&skill).unwrap()
	);
	let pinned = Pinned {
		metadata: skill,
		loaded: false,
		files: vec![],
		instruction_json_len: None,
	};
	assert_eq!(
		pinned_context_reserve(&[pinned]).unwrap(),
		escaped_instruction_len(&expected).unwrap()
	);
}
#[rstest::rstest]
fn loaded_skill_missing_instructions_is_rejected() {
	let pinned = Pinned {
		metadata: metadata(),
		loaded: true,
		files: vec![],
		instruction_json_len: Some(10),
	};
	assert!(matches!(
		pinned_context_reserve(&[pinned]),
		Err(Error::Forbidden)
	));
}
#[rstest::rstest]
fn no_pinned_skills_still_reserves_the_complete_header() {
	assert_eq!(
		pinned_context_reserve(&[]).unwrap(),
		escaped_instruction_len("\nPinned Skills (select by UUID and origin; use skill_load):\n")
			.unwrap()
	);
}
fn metadata() -> SkillMetadata {
	SkillMetadata {
		skill_id: Uuid::new_v4(),
		name: "check".into(),
		description: "Inspect input".into(),
		origin: "test".into(),
		digest: "digest".into(),
		license: None,
	}
}

mod exposure {
	use super::*;
	use crate::ports::capabilities::{
		files::{Action, FileScopePort, Limits as FileLimits, VerifiedFile},
		skills::Limits,
	};
	use aidash_domain::{
		RunControl, RunPhase,
		capabilities::records::Record,
		policy::Resource,
		registry::{EntityRef, Entry, bindings::BindingSnapshot},
	};
	use std::collections::BTreeMap;

	const NODE: &str = "aidash://local";

	struct Scope {
		snapshot: BindingSnapshot,
		pinned: Value,
		files: BTreeMap<String, Vec<u8>>,
		updates: usize,
		required: Vec<String>,
	}
	#[async_trait::async_trait]
	impl FileScopePort for Scope {
		fn local_node(&self) -> &str {
			NODE
		}
		fn binding_snapshot(&self) -> Result<&BindingSnapshot> {
			Ok(&self.snapshot)
		}
		fn limits(&self) -> Result<FileLimits> {
			Ok(FileLimits {
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
		fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
			Resource {
				tenant: "tenant".into(),
				kind: kind.into(),
				id: id.into(),
				attributes,
			}
		}
		async fn entry(&mut self, _: &EntityRef, _: &str) -> Result<Entry> {
			panic!("unexpected entry effect")
		}
		async fn check_pinned(&mut self, _: &Entry) -> Result<()> {
			panic!("unexpected check_pinned effect")
		}
		async fn effective(&mut self, _: &EntityRef) -> Result<Entry> {
			panic!("unexpected effective effect")
		}
		async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
			self.required.push(format!("{}:{action}", resource.id));
			Ok(())
		}
		async fn serialize_sharing(&mut self) -> Result<()> {
			panic!("unexpected serialize_sharing effect")
		}
		async fn for_run(&mut self, _: &RunMetadata) -> Result<Area> {
			panic!("unexpected for_run effect")
		}
		async fn authorize(&mut self, _: &Area, _: &str) -> Result<()> {
			panic!("unexpected authorize effect")
		}
		async fn current_run(&mut self, _: &Area) -> Result<Option<Uuid>> {
			panic!("unexpected current_run effect")
		}
		async fn require_current(&mut self, _: &Area, _: &RunMetadata) -> Result<()> {
			panic!("unexpected require_current effect")
		}
		async fn execute(
			&mut self,
			_: Action<'_>,
			_: &mut Area,
			_: Value,
			_: &str,
		) -> Result<Value> {
			panic!("unexpected execute effect")
		}
		async fn output(&mut self, _: &Area, _: Uuid) -> Result<Option<(String, i64, String)>> {
			panic!("unexpected output effect")
		}
		async fn open(&mut self, _: &FileEntry) -> Result<Box<dyn VerifiedFile>> {
			panic!("unexpected open effect")
		}
		async fn cached(&mut self, _: Uuid, _: &str) -> Result<Option<Value>> {
			panic!("unexpected cached effect")
		}
		async fn cache(&mut self, _: Uuid, _: &str, _: &Value) -> Result<()> {
			panic!("unexpected cache effect")
		}
		async fn begin_pending(&mut self, _: Uuid, _: u64) -> Result<()> {
			panic!("unexpected begin_pending effect")
		}
		async fn read_chunk(&mut self, _: &FileEntry, _: u64) -> Result<Vec<u8>> {
			panic!("unexpected read_chunk effect")
		}
		async fn write_pending(&mut self, _: &[u8]) -> Result<()> {
			panic!("unexpected write_pending effect")
		}
		async fn finish_pending(&mut self, _: &str) -> Result<(Uuid, String)> {
			panic!("unexpected finish_pending effect")
		}
		async fn message(&mut self, _: Uuid, _: Uuid) -> Result<Value> {
			panic!("unexpected message effect")
		}
		async fn documents(&mut self, _: &Entry) -> Result<Value> {
			panic!("unexpected documents effect")
		}
		async fn text_file(
			&mut self,
			_: Uuid,
			_: String,
			_: &str,
			_: FileScope,
			_: Value,
		) -> Result<FileEntry> {
			panic!("unexpected text_file effect")
		}
		async fn previous_manifest(&mut self, _: &Area) -> Result<Value> {
			panic!("unexpected previous_manifest effect")
		}
		async fn supersede(&mut self, _: &Area, _: Uuid) -> Result<()> {
			panic!("unexpected supersede effect")
		}
		async fn persist_manifest(&mut self, _: &Area) -> Result<()> {
			panic!("unexpected persist_manifest effect")
		}
		async fn event(&mut self, _: Uuid, _: &str, _: Value) -> Result<()> {
			panic!("unexpected event effect")
		}
	}
	#[async_trait::async_trait]
	impl SkillScope for Scope {
		fn skill_limits(&self) -> Result<Limits> {
			Ok(Limits {
				files: 64,
				bytes: 256_000,
			})
		}
		async fn read_skill_file(&mut self, file: &FileEntry) -> Result<Vec<u8>> {
			Ok(self.files[&file.path].clone())
		}
		async fn put_skill(&mut self, _: Uuid, _: &[u8]) -> Result<(Uuid, String)> {
			panic!("unexpected put_skill effect")
		}
		async fn skill_record(&mut self, _: Uuid) -> Result<Record> {
			Ok(Record {
				id: Uuid::nil(),
				tenant: "tenant".into(),
				owner: "owner".into(),
				area_id: None,
				kind: "skills".into(),
				state: "ready".into(),
				revision: 0,
				data: self.pinned.clone(),
				expires_at: None,
			})
		}
		async fn insert_skills(&mut self, _: Uuid, _: Uuid, _: Value) -> Result<()> {
			panic!("unexpected insert_skills effect")
		}
		async fn update_skills(&mut self, record: &mut Record) -> Result<()> {
			self.updates += 1;
			self.pinned = record.data.clone();
			Ok(())
		}
		async fn context_authority(&mut self, _: &RunMetadata) -> Result<()> {
			Ok(())
		}
	}

	fn file(path: &str, bytes: &[u8]) -> FileEntry {
		FileEntry {
			file_id: Uuid::new_v4(),
			path: path.into(),
			digest: format!("stored:{path}"),
			size: bytes.len() as u64,
			media_type: "application/octet-stream".into(),
			scope: FileScope::References,
			provenance: json!({}),
		}
	}
	fn scope(deferred: bool, skill: &SkillMetadata) -> Scope {
		let mut root = crate::test_support::agent("agent");
		root.config["instructions"] = json!("");
		root.config["bindings"]
			.as_array_mut()
			.unwrap()
			.push(crate::test_support::binding(
				"source",
				NODE,
				"mounted-skills",
			));
		if deferred {
			root.config["exposure"] = json!({"version":"deferred@1"});
		}
		let source = crate::test_support::entry(
			"mounted-skills",
			"source",
			json!({"schema_version":1,"source":{"adapter":"skill_roots","roots":[".agents/skills"]}}),
		);
		let instructions = b"---\nname: check\n---\nInspect \"input\".";
		let files = BTreeMap::from([
			("SKILL.md".to_owned(), instructions.to_vec()),
			("guide.md".to_owned(), b"guide".to_vec()),
		]);
		let pinned = Pinned {
			loaded: false,
			metadata: skill.clone(),
			files: files
				.iter()
				.map(|(path, bytes)| file(path, bytes))
				.collect(),
			instruction_json_len: None,
		};
		Scope {
			snapshot: crate::test_support::resolve(NODE, &root, false, vec![source]),
			pinned: json!([pinned]),
			files,
			updates: 0,
			required: vec![],
		}
	}
	fn run() -> RunMetadata {
		RunMetadata {
			id: Uuid::new_v4(),
			task_id: Uuid::new_v4(),
			workspace_id: Uuid::new_v4(),
			home_node: NODE.into(),
			agent_id: "agent".into(),
			agent_version: "1.0.0".into(),
			phase: RunPhase::Ready,
			control: RunControl::Active,
			step: 0,
			revision: 0,
			observed_input_seq: 0,
			ledger_worker_ready: true,
			error: None,
			lease_owner: None,
			lease_until: None,
			updated_at: chrono::Utc::now(),
		}
	}

	#[rstest::rstest]
	#[case::legacy(false, 1)]
	#[case::deferred(true, 0)]
	#[tokio::test]
	async fn only_legacy_skill_load_writes_the_pinned_record(
		#[case] deferred: bool,
		#[case] updates: usize,
	) {
		let skill = metadata();
		let mut scope = scope(deferred, &skill);
		let result = invoke(
			&mut scope,
			&run(),
			"skill_load",
			json!({"skill_id":skill.skill_id,"expected_digest":skill.digest}),
		)
		.await
		.unwrap();
		assert_eq!(result["path"], "SKILL.md");
		assert_eq!(scope.updates, updates);
		assert_eq!(scope.pinned[0]["loaded"], !deferred);
	}

	#[tokio::test]
	async fn direct_skills_expose_pinned_records_under_skill_asset_read_authority() {
		let skill = metadata();
		let mut scope = scope(true, &skill);
		let run = run();
		let direct = direct(&mut scope, &run).await.unwrap();
		assert_eq!(direct.len(), 1);
		assert_eq!(direct[0].origin, SkillOriginKind::Root);
		let text = std::str::from_utf8(&scope.files["SKILL.md"])
			.unwrap()
			.to_owned();
		assert_eq!(
			direct[0].body_bytes,
			escaped_instruction_len(&text).unwrap()
		);
		// SKILL.md is the instruction body, reserved for capability_load.
		assert_eq!(direct[0].files.len(), 1);
		assert_eq!(direct[0].files[0]["path"], "guide.md");
		assert_eq!(scope.required.len(), 1);
		assert!(scope.required[0].contains("aidash.skill_asset_read"));
		assert!(scope.required[0].ends_with(":tool.invoke"));
		assert_eq!(
			direct_body(&mut scope, &run, skill.skill_id, &skill.digest)
				.await
				.unwrap(),
			text
		);
		assert!(matches!(
			direct_body(&mut scope, &run, skill.skill_id, "changed").await,
			Err(Error::Conflict(code)) if code == "CAPABILITY_CHANGED"
		));
		assert_eq!(
			direct_file(&mut scope, &run, skill.skill_id, &skill.digest, "guide.md")
				.await
				.unwrap(),
			b"guide"
		);
		for path in ["other.md", "SKILL.md"] {
			assert!(
				matches!(
					direct_file(&mut scope, &run, skill.skill_id, &skill.digest, path).await,
					Err(Error::Invalid(code)) if code == "SKILL_FILE_UNAVAILABLE"
				),
				"{path}"
			);
		}
		assert_eq!(scope.updates, 0);
	}

	#[rstest::rstest]
	#[case::legacy(false, "aidash.skill_list")]
	#[case::deferred(true, "aidash.skill_asset_read")]
	#[tokio::test]
	async fn skill_context_authorizes_through_the_policy_support_binding(
		#[case] deferred: bool,
		#[case] support: &str,
	) {
		let skill = metadata();
		let mut scope = scope(deferred, &skill);
		let text = context(&mut scope, &run()).await.unwrap();
		assert!(text.contains(&skill.skill_id.to_string()));
		assert_eq!(scope.required.len(), 1);
		assert!(scope.required[0].contains(support), "{:?}", scope.required);
	}

	#[tokio::test]
	async fn legacy_runs_have_no_direct_skill_catalog_input() {
		let skill = metadata();
		let mut scope = scope(false, &skill);
		assert!(direct(&mut scope, &run()).await.unwrap().is_empty());
		assert!(scope.required.is_empty());
	}
}
