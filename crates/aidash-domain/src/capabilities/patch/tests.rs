use super::*;
use rstest::rstest;
#[test]
fn strict_grammar_accepts_add_update_and_delete_in_one_batch() {
	let edits=parse("*** Begin Patch\n*** Add File: new.txt\n+new\n*** Update File: existing.txt\n@@\n-old\n+changed\n*** Delete File: obsolete.txt\n*** End Patch").unwrap();
	assert_eq!(edits.len(), 3);
	assert_eq!(edits[0].path, "new.txt");
	assert!(matches!(&edits[0].change,Change::Add(text) if text=="new\n"));
	assert!(matches!(&edits[2].change, Change::Delete));
	let Change::Update(chunks) = edits.into_iter().nth(1).unwrap().change else {
		panic!("expected update");
	};
	assert_eq!(
		update("old\n", chunks, "existing.txt").unwrap(),
		"changed\n"
	);
}
#[rstest]
#[case("*** Begin Patch\n*** End Patch")]
#[case("*** Add File: file.txt\n+text\n*** End Patch")]
#[case("*** Begin Patch\n*** Add File: file.txt\ntext\n*** End Patch")]
#[case("*** Begin Patch\n*** Update File: file.txt\n@@\n*** End Patch")]
fn malformed_grammar_is_rejected(#[case] input: &str) {
	assert_eq!(
		parse(input).err().unwrap().to_string(),
		"INVALID_PATCH_GRAMMAR"
	);
}
#[test]
fn repeated_paths_cannot_bypass_batch_preconditions() {
	let error = parse(
		"*** Begin Patch\n*** Delete File: file.txt\n*** Delete File: file.txt\n*** End Patch",
	)
	.err()
	.unwrap();
	assert_eq!(error.to_string(), "DUPLICATE_PATCH_PATH");
}
#[test]
fn unmatched_hunks_reject_the_whole_update() {
	let edits = parse(
		"*** Begin Patch\n*** Update File: file.txt\n@@\n-missing\n+replacement\n*** End Patch",
	)
	.unwrap();
	let Change::Update(chunks) = edits.into_iter().next().unwrap().change else {
		panic!("expected update");
	};
	let error = update("original\n", chunks, "file.txt").unwrap_err();
	assert_eq!(error.to_string(), "PATCH_HUNK: file.txt");
}
