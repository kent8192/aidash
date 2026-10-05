use super::*;
fn file() -> FileEntry {
	FileEntry {
		file_id: Uuid::new_v4(),
		path: "report.txt".into(),
		digest: "digest".into(),
		size: 5,
		media_type: "text/plain".into(),
		scope: FileScope::Working,
		provenance: json!({}),
	}
}
fn limits() -> Limits {
	Limits {
		files: 4,
		file_bytes: 8,
		bytes: 12,
		admission: true,
	}
}
fn select(file: &FileEntry) -> Selection {
	Selection {
		file_id: file.file_id,
		expected_digest: file.digest.clone(),
	}
}
#[rstest::rstest]
fn selected_order_and_immutable_metadata_are_preserved() {
	let a = file();
	let mut b = file();
	b.path = "other.txt".into();
	let (selected, bytes) = select_files(
		&[a.clone(), b.clone()],
		&[select(&b), select(&a)],
		&limits(),
	)
	.unwrap();
	assert_eq!(
		selected.iter().map(|f| f.file_id).collect::<Vec<_>>(),
		vec![b.file_id, a.file_id]
	);
	assert_eq!(bytes, 10);
	assert_eq!(selected[0].digest, b.digest);
}
#[rstest::rstest]
fn missing_selection_remains_concealed() {
	let f = file();
	assert!(matches!(
		select_files(&[], &[select(&f)], &limits()),
		Err(Error::NotFound(_))
	));
}
#[rstest::rstest]
fn changed_digest_cannot_be_snapshotted() {
	let f = file();
	let mut selection = select(&f);
	selection.expected_digest = "old".into();
	assert!(
		matches!(select_files(&[f],&[selection],&limits()),Err(Error::Conflict(code)) if code=="FILE_CHANGED: report.txt")
	);
}
#[rstest::rstest]
fn duplicate_selection_is_rejected() {
	let f = file();
	assert!(
		matches!(select_files(std::slice::from_ref(&f),&[select(&f),select(&f)],&limits()),Err(Error::Invalid(code)) if code=="SHARE_FILE_LIMIT")
	);
}
#[rstest::rstest]
fn a_file_cannot_exceed_its_budget() {
	let mut f = file();
	f.size = 9;
	assert!(
		matches!(select_files(&[f.clone()],&[select(&f)],&limits()),Err(Error::Invalid(code)) if code=="SHARE_FILE_LIMIT")
	);
}
#[rstest::rstest]
fn the_complete_snapshot_must_fit_the_total_budget() {
	let a = file();
	let b = file();
	let mut l = limits();
	l.bytes = 9;
	assert!(
		matches!(select_files(&[a.clone(),b.clone()],&[select(&a),select(&b)],&l),Err(Error::Invalid(code)) if code=="SHARE_BYTE_LIMIT")
	);
}
