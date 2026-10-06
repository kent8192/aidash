use super::*;
use crate::marketplace::installations;
use crate::marketplace::installations::tests::{input, published, validation};
use chrono::{TimeZone, Utc};
use rstest::rstest;
use serde_json::json;
use uuid::Uuid;
fn event(kind: &str, data: serde_json::Value) -> Event {
	Event {
		sequence: 17,
		id: Uuid::nil(),
		node_id: "node".into(),
		workspace_id: None,
		kind: kind.into(),
		data,
		created_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
	}
}
#[rstest]
#[tokio::test]
async fn audit_events_are_never_visible_and_do_not_read_distribution_state() {
	let mut scope = published();
	let visible = visible(
		&mut scope,
		&event("marketplace.audit", json!({"key":"package"})),
		"node",
	)
	.await
	.unwrap();
	assert!(!visible);
	assert_eq!(scope.calls, Vec::<String>::new());
}
#[rstest]
#[tokio::test]
async fn disabled_compatibility_hides_events_before_loading_private_resources() {
	let mut scope = published();
	scope.denied = Some("compatibility_ready".into());
	let visible = visible(
		&mut scope,
		&event("marketplace.published", json!({"key":"package"})),
		"node",
	)
	.await
	.unwrap();
	assert!(!visible);
	assert_eq!(scope.calls, vec!["compatibility_ready"]);
}
#[rstest]
#[tokio::test]
async fn distribution_connection_failure_is_preserved_for_the_retry_path() {
	let mut scope = published();
	scope.denied = Some("compatibility_external".into());
	let result = visible(
		&mut scope,
		&event("marketplace.published", json!({"key":"package"})),
		"node",
	)
	.await;
	assert!(
		matches!(result,Err(Error::External(message)) if message=="fixture connection failure")
	);
}
#[rstest]
#[case::foreign_installation(json!({"installation":"private-installation","tenant":"other","revision":1}))]
#[case::missing_reference(json!({"tenant":"owner"}))]
#[tokio::test]
async fn foreign_or_unbound_events_do_not_load_installation_documents(
	#[case] data: serde_json::Value,
) {
	let mut scope = published();
	let visible = visible(&mut scope, &event("marketplace.installed", data), "node")
		.await
		.unwrap();
	assert!(!visible);
	assert_eq!(scope.calls, vec!["compatibility_ready"]);
}
#[rstest]
#[case::published("marketplace.published", true)]
#[case::shared("marketplace.distribution_changed", true)]
#[case::other("marketplace.other", false)]
#[tokio::test]
async fn recipient_frames_preserve_cursor_envelope_and_redact_legacy_publisher_subject(
	#[case] kind: &str,
	#[case] redact: bool,
) {
	// Arrange
	let mut scope = published();
	let source = event(kind, json!({"key":"package","actor":"private-subject"}));
	// Act
	let frame = frame(&mut scope, &source, "node").await.unwrap().unwrap();
	let received: serde_json::Value = serde_json::from_str(&frame).unwrap();
	// Assert
	assert_eq!(received["sequence"], 17);
	assert_eq!(received["id"], source.id.to_string());
	assert_eq!(received["type"], kind);
	assert_eq!(received["data"].get("actor").is_none(), redact);
	assert_eq!(source.data["actor"], "private-subject");
}
#[rstest]
#[tokio::test]
async fn oversized_utf8_recipient_frame_is_rejected_after_current_authorization() {
	let mut scope = published();
	let source = event(
		"marketplace.published",
		json!({"key":"package","content":"💡".repeat(16_384)}),
	);
	let result = frame(&mut scope, &source, "node").await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert!(scope.calls.iter().any(|c| c == "marketplace.read"));
}
#[rstest]
#[case::read_denied("workspace.read",vec!["workspace.read"])]
#[case::events_denied("workspace.events",vec!["workspace.read","workspace.events"])]
#[case::resource_denied("workspace_event_visible",vec!["workspace.read","workspace.events","workspace_event_visible"])]
#[tokio::test]
async fn polling_applies_workspace_and_event_authority_in_order(
	#[case] denied: &str,
	#[case] expected_calls: Vec<&str>,
) {
	let mut scope = published();
	scope.denied = Some(denied.into());
	let mut source = event("workspace.changed", json!({}));
	source.workspace_id = Some(Uuid::nil());
	let result = poll_events(&mut scope, vec![source], "node").await.unwrap();
	assert_eq!(result.len(), 0);
	assert_eq!(scope.calls, expected_calls);
}
#[rstest]
#[tokio::test]
async fn polling_keeps_event_order_and_hides_operator_audit() {
	let mut scope = published();
	let mut workspace = event("workspace.changed", json!({}));
	workspace.workspace_id = Some(Uuid::nil());
	let result = poll_events(
		&mut scope,
		vec![
			event("marketplace.audit", json!({})),
			workspace,
			event(
				"marketplace.published",
				json!({"key":"package","actor":"private-subject"}),
			),
		],
		"node",
	)
	.await
	.unwrap();
	assert_eq!(
		result.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
		vec!["workspace.changed", "marketplace.published"]
	);
	assert_eq!(result[1].data, json!({"key":"package"}));
}
#[rstest]
#[case::retained_single(true, 1)]
#[case::inactive_list(false, 0)]
#[tokio::test]
async fn exact_retained_revision_remains_readable_without_following_active_alias(
	#[case] single: bool,
	#[case] expected: usize,
) {
	// Arrange: retained revision one has individual authority; selected pointer is two.
	let mut scope = published();
	let request = input(&scope);
	let installed = installations::install(&mut scope, &validation(), "package", &request, "node")
		.await
		.unwrap();
	scope
		.installations
		.get_mut(&installed.installation.id)
		.unwrap()
		.active_revision = Some(2);
	scope.calls.clear();
	// Act
	let result = registry_entries(&mut scope, vec![installed.entry], single, "node")
		.await
		.unwrap();
	// Assert: single reads retain exact content, discovery excludes inactive aliases.
	assert_eq!(result.len(), expected);
	assert_eq!(
		scope.calls.iter().any(|c| c == "selected_installation"),
		!single
	);
}
#[rstest]
#[tokio::test]
async fn disabled_gate_blocks_retained_registry_disclosure_before_catalog_read() {
	let mut scope = published();
	let request = input(&scope);
	let installed = installations::install(&mut scope, &validation(), "package", &request, "node")
		.await
		.unwrap();
	scope.calls.clear();
	scope.denied = Some("compatibility_ready".into());
	let result = registry_entries(&mut scope, vec![installed.entry], true, "node").await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(scope.calls, vec!["compatibility_ready"]);
}
#[rstest]
#[tokio::test]
async fn state_keeps_previously_authorized_workspace_events_while_hiding_marketplace_audit() {
	let mut scope = published();
	scope.denied = Some("workspace.read".into());
	let result = state_events(
		&mut scope,
		vec![
			event("workspace.changed", json!({})),
			event("marketplace.audit", json!({})),
		],
		"node",
	)
	.await
	.unwrap();
	assert_eq!(
		result.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
		vec!["workspace.changed"]
	);
	assert_eq!(scope.calls, Vec::<String>::new());
}
