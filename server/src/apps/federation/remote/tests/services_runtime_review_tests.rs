use super::*;

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
