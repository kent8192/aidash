use super::*;
use rstest::rstest;

fn file(path: &str, size: u64) -> File {
	File {
		path: path.into(),
		digest: "a".repeat(64),
		size,
		media_type: "text/plain".into(),
	}
}
fn share() -> Limits {
	Limits {
		files: 64,
		file_bytes: 100 * 1024 * 1024,
		bytes: 256 * 1024 * 1024,
	}
}
fn message(descriptions: &[(&str, &str)]) -> Purpose {
	Purpose::Message {
		content: "done".into(),
		descriptions: descriptions
			.iter()
			.map(|(path, text)| ((*path).into(), (*text).into()))
			.collect(),
	}
}

#[rstest]
fn a_file_share_keeps_the_deployments_share_limits() {
	let files = vec![
		file("a.txt", 100 * 1024 * 1024),
		file("b.txt", 100 * 1024 * 1024),
	];
	assert_eq!(
		validate(&Purpose::FileShare, &files, share()).unwrap(),
		200 * 1024 * 1024
	);
}
#[rstest]
#[case::empty(vec![], "SHARE_FILE_LIMIT")]
#[case::too_large(vec![file("a.txt", 100 * 1024 * 1024 + 1)], "SHARE_FILE_LIMIT")]
#[case::duplicate(vec![file("a.txt", 1), file("a.txt", 1)], "SHARE_FILE_LIMIT")]
fn a_file_share_over_a_budget_is_refused(#[case] files: Vec<File>, #[case] code: &str) {
	assert!(
		matches!(validate(&Purpose::FileShare, &files, share()), Err(Error::Invalid(got)) if got == code)
	);
}
#[rstest]
fn more_than_sixty_four_files_are_refused() {
	let files: Vec<File> = (0..65).map(|i| file(&format!("f{i}.txt"), 1)).collect();
	assert!(matches!(
		validate(&Purpose::FileShare, &files, share()),
		Err(Error::Invalid(_))
	));
}
#[rstest]
fn the_total_may_not_pass_the_byte_budget() {
	let files = vec![
		file("a", 100 * 1024 * 1024),
		file("b", 100 * 1024 * 1024),
		file("c", 100 * 1024 * 1024),
	];
	assert!(
		matches!(validate(&Purpose::FileShare, &files, share()), Err(Error::Invalid(got)) if got == "SHARE_BYTE_LIMIT")
	);
}
#[rstest]
#[case::path("../a.txt")]
#[case::absolute("/a.txt")]
#[case::backslash("a\\b")]
fn a_path_that_leaves_its_directory_is_refused(#[case] path: &str) {
	assert!(validate(&Purpose::FileShare, &[file(path, 1)], share()).is_err());
}
#[rstest]
fn a_digest_must_be_lowercase_sha256_hex() {
	let mut bad = file("a.txt", 1);
	bad.digest = "A".repeat(64);
	assert!(validate(&Purpose::FileShare, &[bad], share()).is_err());
}
#[rstest]
fn a_message_keeps_the_local_attachment_limits() {
	let nine: Vec<File> = (0..9).map(|i| file(&format!("f{i}.txt"), 1)).collect();
	assert!(validate(&message(&[]), &nine, share()).is_err());
	let big = vec![file("a.txt", MESSAGE_BYTES + 1)];
	assert!(validate(&message(&[]), &big, share()).is_err());
	assert_eq!(
		validate(&message(&[]), &[file("a.txt", MESSAGE_BYTES)], share()).unwrap(),
		MESSAGE_BYTES
	);
}
#[rstest]
fn a_message_describes_each_image_and_bounds_its_text() {
	let mut picture = file("p.png", 3);
	picture.media_type = "image/png".into();
	assert!(validate(&message(&[]), std::slice::from_ref(&picture), share()).is_err());
	assert!(
		validate(
			&message(&[("p.png", "a chart")]),
			std::slice::from_ref(&picture),
			share()
		)
		.is_ok()
	);
	// Files may travel without text, but the text is bounded as a message's is.
	let blank = Purpose::Message {
		content: String::new(),
		descriptions: BTreeMap::new(),
	};
	assert!(validate(&blank, &[file("a.txt", 1)], share()).is_ok());
	let long = Purpose::Message {
		content: "x".repeat(64_001),
		descriptions: BTreeMap::new(),
	};
	assert!(validate(&long, &[file("a.txt", 1)], share()).is_err());
	// A description of a file that is not attached has nothing to describe.
	assert!(
		validate(
			&message(&[("other.png", "x")]),
			&[file("a.txt", 1)],
			share()
		)
		.is_err()
	);
}
#[rstest]
fn the_same_key_names_the_same_delivery_and_another_admission_names_another() {
	let admission = Uuid::new_v4();
	assert_eq!(delivery_id(admission, "k"), delivery_id(admission, "k"));
	assert_ne!(delivery_id(admission, "k"), delivery_id(admission, "l"));
	assert_ne!(
		delivery_id(admission, "k"),
		delivery_id(Uuid::new_v4(), "k")
	);
}
#[rstest]
fn the_input_digest_binds_the_purpose_the_manifest_and_the_grant() {
	let (grant, admission) = (Uuid::new_v4(), Uuid::new_v4());
	let files = vec![file("a.txt", 1)];
	let base = input_digest(grant, admission, "k", &Purpose::FileShare, &files);
	assert_eq!(
		base,
		input_digest(grant, admission, "k", &Purpose::FileShare, &files)
	);
	assert_ne!(
		base,
		input_digest(grant, admission, "k", &message(&[]), &files)
	);
	assert_ne!(
		base,
		input_digest(
			grant,
			admission,
			"k",
			&Purpose::FileShare,
			&[file("b.txt", 1)]
		)
	);
	assert_ne!(
		base,
		input_digest(Uuid::new_v4(), admission, "k", &Purpose::FileShare, &files)
	);
}
#[rstest]
fn the_manifest_is_deterministic_so_its_digest_survives_a_retry() {
	let id = Uuid::new_v4();
	let files = vec![file("a.txt", 1), file("b.txt", 2)];
	let one = manifest(id, &files);
	let two = manifest(id, &files);
	assert_eq!(one[0].file_id, two[0].file_id);
	assert_ne!(one[0].file_id, one[1].file_id);
	assert_eq!(one[1].path, "b.txt");
	assert_eq!(one[1].scope, FileScope::Received);
}

#[rstest]
fn each_part_of_a_deliverys_effect_has_its_own_stable_key() {
	let id = Uuid::new_v4();
	assert_eq!(part_key(id, 0), part_key(id, 0));
	assert_ne!(part_key(id, 0), part_key(id, 1));
	assert_ne!(part_key(id, 0), part_key(Uuid::new_v4(), 0));
	assert_ne!(part_key(id, 0), id);
}

#[rstest]
fn only_the_committed_receipt_of_this_delivery_with_every_file_confirms_it() {
	let id = Uuid::new_v4();
	let files = vec![file("a.txt", 3), file("b.txt", 4)];
	let reply = |state: &str, id: Uuid, files: serde_json::Value| json!({"state":state,"transfer_id":id,"receipt":{"files":files}});
	let both = json!([
		{"digest":"a".repeat(64),"size":3},
		{"digest":"a".repeat(64),"size":4},
	]);
	assert!(receipt_matches(
		id,
		&files,
		&reply("committed", id, both.clone())
	));
	assert!(!receipt_matches(
		id,
		&files,
		&reply("staging", id, both.clone())
	));
	assert!(!receipt_matches(
		id,
		&files,
		&reply("committed", Uuid::new_v4(), both)
	));
	let short = json!([{"digest":"a".repeat(64),"size":3}]);
	assert!(!receipt_matches(id, &files, &reply("committed", id, short)));
	let altered = json!([
		{"digest":"a".repeat(64),"size":3},
		{"digest":"b".repeat(64),"size":4},
	]);
	assert!(!receipt_matches(
		id,
		&files,
		&reply("committed", id, altered)
	));
	assert!(!receipt_matches(
		id,
		&files,
		&json!({"state":"committed","transfer_id":id})
	));
}
