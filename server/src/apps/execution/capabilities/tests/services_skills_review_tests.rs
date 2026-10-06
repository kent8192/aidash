//! Unit tests for services::skills.
use super::*;

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
