use super::*;
use rstest::{fixture, rstest};
#[fixture]
fn entry() -> Entry {
	serde_json::from_value(json!({"id":"managed","version":"1.0.0","kind":"agent","name":{},"description":{},"config":{"model":{"id":"model","version":"1.0.0"},"tools":[],"skills":[],"cluster":null}})).unwrap()
}
#[rstest]
fn new_drafts_disable_unspecified_capabilities(mut entry: Entry) {
	new_draft_defaults(&mut entry).unwrap();
	for key in [
		"allow_task_creation",
		"allow_task_delegation",
		"allow_memory_write",
		"allow_workspace_retrieval",
		"allow_cross_conversation_memory",
	] {
		assert_eq!(entry.config[key], json!(false));
	}
}
#[rstest]
fn explicit_capability_choices_survive_defaults(mut entry: Entry) {
	entry.config["allow_task_delegation"] = json!(true);
	new_draft_defaults(&mut entry).unwrap();
	assert_eq!(entry.config["allow_task_delegation"], json!(true));
}
#[rstest]
#[case::null(json!(null))]
#[case::string(json!("false"))]
#[case::number(json!(0))]
fn invalid_capability_types_are_rejected(mut entry: Entry, #[case] value: Value) {
	entry.config["allow_task_creation"] = value;
	assert_eq!(
		new_draft_defaults(&mut entry).unwrap_err(),
		Error::Invalid("allow_task_creation must be a boolean".into())
	);
}
#[rstest]
#[case::tool("tool", "managed")]
#[case::missing("agent", "")]
fn content_requires_a_managed_agent(mut entry: Entry, #[case] kind: &str, #[case] id: &str) {
	entry.kind = kind.into();
	entry.id = id.into();
	assert_eq!(
		check_content(&entry, &[], "").unwrap_err(),
		Error::Invalid("draft must contain a managed agent identity".into())
	);
}
#[rstest]
fn release_note_limit_counts_utf8_bytes(entry: Entry) {
	let notes = "é".repeat(4096);
	assert!(check_content(&entry, &[], &notes).is_ok());
	assert_eq!(
		check_content(&entry, &[], &(notes + "a")).unwrap_err(),
		Error::Invalid("release notes exceed 8 KiB".into())
	);
}
#[rstest]
fn malformed_agent_config_is_invalid(mut entry: Entry) {
	entry.config = json!({});
	assert!(matches!(
		check_content(&entry, &[], ""),
		Err(Error::Invalid(_))
	));
}
#[rstest]
fn document_changes_invalidate_previous_sharing_acknowledgement() {
	let documents = json!([{ "id":"doc","text":"old" }]);
	let share = Some((true, digest(&documents)));
	assert!(current_share(&documents, share.clone()).is_some());
	assert!(current_share(&json!([{ "id":"doc","text":"new" }]), share).is_none());
}
