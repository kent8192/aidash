use super::*;

#[rstest::rstest]
fn remote_workspace_chunk_bad_requests_keep_the_tool_error_type() {
	let request = json!({"operation":"workspace_record_chunk"});
	let error = json!({"error":"workspace record offset out of range"});
	assert!(matches!(
		map_workspace_chunk_bad_request("/workspace", Some(&request), &error),
		Some(Error::Invalid(message)) if message == "workspace record offset out of range"
	));
	assert!(map_workspace_chunk_bad_request("/discover", Some(&request), &error).is_none());
	assert!(map_workspace_chunk_bad_request("/workspace", None, &error).is_none());
}

#[rstest::rstest]
fn child_task_summary_is_independent_of_child_payload_sizes() {
	let mut summary = ChildTaskSummary {
		has_pending: false,
		has_failed: false,
	};
	for status in [
		crate::domain::TaskStatus::Running,
		crate::domain::TaskStatus::Failed,
	] {
		summary.include_status(status);
	}
	assert!(summary.has_pending && summary.has_failed);
	assert!(serde_json::to_vec(&summary).unwrap().len() < 64);
}
