use super::*;
use rstest::{fixture, rstest};
#[fixture]
fn entry() -> Entry {
	serde_json::from_value(json!({"id":"managed","version":"1.0.0","kind":"agent","name":{},"description":{},"config":{"model":{"id":"model","version":"1.0.0"},"schema_version":1,"instructions":"","bindings":[],"remove_default":[],"cluster":null}})).unwrap()
}
#[rstest]
fn new_drafts_record_the_binding_schema_without_legacy_flags(mut entry: Entry) {
	entry.config = json!({"model":{"id":"model","version":"1.0.0"}});
	new_draft_defaults(&mut entry).unwrap();
	assert_eq!(entry.config["schema_version"], 1);
	assert_eq!(entry.config["bindings"], json!([]));
	assert_eq!(entry.config["remove_default"], json!([]));
	assert!(entry.config.get("allow_task_creation").is_none());
}
#[rstest]
fn authored_bindings_and_removed_defaults_survive_draft_defaults(mut entry: Entry) {
	entry.config["remove_default"] = json!(["memory_write"]);
	let before = entry.config.clone();
	new_draft_defaults(&mut entry).unwrap();
	assert_eq!(entry.config, before);
}
#[rstest]
fn unsupported_binding_schema_is_rejected(mut entry: Entry) {
	entry.config["schema_version"] = json!(2);
	assert!(new_draft_defaults(&mut entry).is_err());
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
