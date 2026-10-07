//! Fixed-time generation fixtures shared by use-case regressions.
use aidash_domain::generation::requests::Request;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use uuid::Uuid;
pub(super) fn request(id: u128) -> Request {
	let timestamp = DateTime::<Utc>::from_timestamp(1000, 0).unwrap();
	Request {
		id: Uuid::from_u128(id),
		tenant: "tenant".into(),
		policy_id: "policy".into(),
		policy_revision: 7,
		task_id: Uuid::from_u128(2),
		home_node: String::new(),
		foreign_intent: None,
		prepared: true,
		grant_id: None,
		admission_id: None,
		workspace_id: Uuid::from_u128(3),
		credential_id: Uuid::from_u128(4),
		root_subject: "alice".into(),
		subject_chain: vec!["alice".into()],
		agent_id: "agent".into(),
		agent_version: "1.0.0".into(),
		definition: json!({}),
		status: "ACTIVE".into(),
		reason: "work".into(),
		depth: 1,
		token_limit: 10000,
		quota_released: false,
		expires_at: timestamp,
		created_at: timestamp,
	}
}
pub(super) fn specification() -> Value {
	let document = json!({"enabled":true,"template":{"id":"template","version":"1.0.0","kind":"agent","name":{"en":"Template"},"description":{"en":""},"config":{"schema_version":1,"bindings":[],"remove_default":[],"model":{"id":"model","version":"1.0.0"},"instructions":"Do work."}},"permissions":{"roles":[],"groups":[],"attributes":{}},"limits":{"max_agents":4,"max_concurrent":2,"max_depth":2,"token_budget":800000,"tokens_per_agent":200000,"lifetime_seconds":3600},"approval_required":true});
	serde_json::to_value(
		serde_json::from_value::<aidash_domain::generation::policy::Spec>(document).unwrap(),
	)
	.unwrap()
}
