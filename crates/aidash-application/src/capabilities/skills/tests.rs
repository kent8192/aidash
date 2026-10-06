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
