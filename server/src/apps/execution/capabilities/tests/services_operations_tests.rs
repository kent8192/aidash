//! Unit tests for services::operations.
use super::*;

fn file(scope: FileScope, path: &str) -> FileEntry {
	FileEntry {
		file_id: Uuid::new_v4(),
		path: path.into(),
		digest: "digest".into(),
		size: 1,
		media_type: "text/plain".into(),
		scope,
		provenance: Value::Null,
	}
}

#[test]
fn rejects_duplicate_mounted_paths_within_a_scope() {
	assert!(has_input_path_collision(&[
		file(FileScope::References, "_skills/one/SKILL.md"),
		file(FileScope::References, "_skills/one/SKILL.md"),
	]));
	assert!(!has_input_path_collision(&[
		file(FileScope::References, "shared.txt"),
		file(FileScope::Working, "shared.txt"),
	]));
}

#[test]
fn accepts_runner_page_rounding_for_working_limits() {
	assert_eq!(rounded_working_bytes(4097, 4096), Some(8192));
	assert_eq!(rounded_working_bytes(0, 4096), Some(4096));
	assert_eq!(rounded_working_bytes(4097, 0), None);
}
