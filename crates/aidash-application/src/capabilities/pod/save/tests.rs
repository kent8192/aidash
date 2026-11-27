use super::*;
use uuid::Uuid;

fn registered(path: &str, digest: &str, size: u64) -> MountedFile {
	MountedFile {
		file_id: Uuid::new_v4(),
		path: path.into(),
		digest: digest.into(),
		size,
		media_type: "text/plain".into(),
		scope: FileScope::Working,
		provenance: json!({}),
	}
}

fn found(path: &str, digest: &str, size: u64) -> (String, ManifestFile) {
	(
		path.into(),
		ManifestFile {
			path: path.into(),
			size,
			sha256: digest.into(),
		},
	)
}

#[test]
fn unchanged_files_are_kept_and_changed_ones_replaced() {
	let plan = plan(
		vec![
			registered("a.txt", "same", 1),
			registered("b.txt", "old", 1),
		],
		BTreeMap::from([found("a.txt", "same", 1), found("b.txt", "new", 2)]),
		&[".".into()],
		false,
		100,
		0,
	)
	.unwrap();
	assert_eq!(plan.unchanged, ["a.txt"]);
	assert_eq!(
		plan.saved
			.iter()
			.map(|f| f.path.as_str())
			.collect::<Vec<_>>(),
		["b.txt"]
	);
	assert!(plan.removed.is_empty());
	assert_eq!(plan.kept.len(), 1);
}

#[test]
fn sync_removes_registered_files_under_the_paths_that_the_selection_no_longer_holds() {
	let plan = plan(
		vec![
			registered("src/gone.rs", "x", 1),
			registered("docs/keep.md", "y", 1),
		],
		BTreeMap::from([found("src/main.rs", "z", 1)]),
		&["src".into()],
		true,
		100,
		0,
	)
	.unwrap();
	assert_eq!(plan.removed, ["src/gone.rs"]);
	assert_eq!(plan.kept[0].path, "docs/keep.md");
}

#[test]
fn without_sync_nothing_outside_the_selection_is_removed() {
	let plan = plan(
		vec![registered("src/gone.rs", "x", 1)],
		BTreeMap::from([found("src/main.rs", "z", 1)]),
		&["src".into()],
		false,
		100,
		0,
	)
	.unwrap();
	assert!(plan.removed.is_empty());
	assert_eq!(plan.kept.len(), 1);
}

#[test]
fn a_selection_over_the_byte_limit_is_refused_whole_naming_the_first_path_over_it() {
	let error = plan(
		vec![registered("kept.bin", "k", 40)],
		BTreeMap::from([
			found("a.bin", "a", 30),
			found("b.bin", "b", 30),
			found("c.bin", "c", 30),
		]),
		&[".".into()],
		false,
		100,
		10,
	)
	.unwrap_err();
	let Error::Conflict(message) = error else {
		panic!("expected a refusal");
	};
	assert!(message.starts_with("SAVE_LIMIT: saving b.bin"), "{message}");
	assert!(message.contains("100 bytes"));
}

#[test]
fn a_selection_over_the_file_limit_is_refused_naming_the_first_path_over_it() {
	let existing = (0..MAX_AREA_FILES)
		.map(|n| registered(&format!("f{n}"), "d", 0))
		.collect();
	let error = plan(
		existing,
		BTreeMap::from([found("new.txt", "n", 0)]),
		&["new.txt".into()],
		false,
		u64::MAX,
		0,
	)
	.unwrap_err();
	let Error::Conflict(message) = error else {
		panic!("expected a refusal");
	};
	assert!(message.contains("new.txt"));
	assert!(message.contains(&MAX_AREA_FILES.to_string()));
}

#[test]
fn a_reference_file_is_never_replaced() {
	let mut reference = registered("refs/spec.md", "r", 1);
	reference.scope = FileScope::References;
	reference.provenance = json!({"kind": "reference", "id": Uuid::new_v4()});
	let error = plan(
		vec![reference],
		BTreeMap::from([found("refs/spec.md", "other", 1)]),
		&["refs".into()],
		false,
		100,
		0,
	)
	.unwrap_err();
	assert!(matches!(error, Error::Conflict(message) if message.starts_with("FILE_EXISTS")));
}

#[test]
fn roots_accept_the_whole_scratch_space_and_refuse_escapes() {
	assert_eq!(roots(&[".".into()]).unwrap(), ["."]);
	assert_eq!(roots(&["src/".into()]).unwrap(), ["src"]);
	assert!(roots(&["../x".into()]).is_err());
	assert!(roots(&[]).is_err());
	assert!(under("src/a.rs", "src"));
	assert!(!under("srcs/a.rs", "src"));
	assert!(under("anything", "."));
}

#[test]
fn a_saved_image_records_its_image_type_so_a_model_can_read_it() {
	assert_eq!(
		media_type("preview.bin", b"\x89PNG\r\n\x1a\n\0\0\0\r"),
		"image/png"
	);
	assert_eq!(media_type("preview.MP4", b""), "video/mp4");
	assert_eq!(
		media_type("notes.txt", b"plain text file"),
		"application/octet-stream"
	);
}
