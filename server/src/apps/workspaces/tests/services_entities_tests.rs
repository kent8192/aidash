use super::*;
use chrono::Utc;
use uuid::Uuid;
#[rstest::rstest]
fn task_transitions_protect_terminal_states() {
	assert!(TaskStatus::Open.can_transition(&TaskStatus::Claimed));
	assert!(!TaskStatus::Open.can_transition(&TaskStatus::Completed));
	assert!(!TaskStatus::Completed.can_transition(&TaskStatus::Cancelled));
	assert!(TaskStatus::Running.can_transition(&TaskStatus::Blocked));
}

#[rstest::rstest]
fn run_from_old_peer_defaults_observed_input_sequence() {
	let id = Uuid::new_v4();
	let wire = json!({
		"id":id,"task_id":id,"workspace_id":id,"home_node":"aidash://old",
		"agent_id":"research","agent_version":"1.0.0","phase":"THINKING",
		"control":"ACTIVE","context":{},"pending":{},"step":0,"revision":0,
		"error":null,"lease_owner":null,"lease_until":null,"updated_at":Utc::now()
	});
	let run: Run = serde_json::from_value(wire).unwrap();
	assert_eq!(run.observed_input_seq, 0);
}
