use super::*;

#[test]
fn a_path_a_source_would_refuse_is_not_reported_as_added() {
	assert_eq!(
		prepare_batch_file("plan/api.md.extracted.txt", "aGk=", 1024),
		Err("INVALID_PATH")
	);
	assert_eq!(
		prepare_batch_file("plan/api.md", "aGk=", 1024),
		Ok(b"hi".to_vec())
	);
}
