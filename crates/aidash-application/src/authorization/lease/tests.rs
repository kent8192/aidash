use super::*;
use crate::{Error, authorization::Snapshot};
use aidash_domain::policy::{Decision, Evaluation};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::json;
struct Lease {
	snapshot: Snapshot,
	subjects: Vec<String>,
	environment: Value,
	records: Vec<(Evaluation, Decision)>,
	fail: bool,
}
#[fixture]
fn lease() -> Lease {
	Lease{snapshot:Snapshot{revision:23,bundle:serde_json::from_value(json!({"tenant":"tenant","subjects":{"reader":{"kind":"user"},"delegate":{"kind":"agent"},"revoked":{"kind":"agent","enabled":false}},"policies":[{"id":"read","effect":"allow","subjects":{"any":true},"actions":["workspace.read"],"resources":{"kinds":["workspace"]}}]})).unwrap()},subjects:vec!["reader".into(),"delegate".into()],environment:json!({"transport":"worker","node_id":"aidash://local"}),records:vec![],fail:false}
}
#[async_trait]
impl AuthorizationLease for Lease {
	fn snapshot(&self) -> &Snapshot {
		&self.snapshot
	}
	fn subjects(&self) -> &[String] {
		&self.subjects
	}
	fn environment(&self) -> &Value {
		&self.environment
	}
	async fn record(&mut self, records: &[(Evaluation, Decision)]) -> Result<()> {
		self.records.extend_from_slice(records);
		if self.fail {
			Err(Error::Port(Box::new(std::io::Error::other("audit fault"))))
		} else {
			Ok(())
		}
	}
}
#[rstest]
#[tokio::test]
async fn all_delegated_subjects_are_evaluated_and_audited_at_the_current_revision(
	mut lease: Lease,
) {
	let resource = resource(
		"tenant",
		&json!({}),
		"workspace",
		"1",
		json!({"owner":"saved-owner"}),
	);
	assert!(
		decide(&mut lease, &resource, "workspace.read")
			.await
			.unwrap()
	);
	assert_eq!(
		lease
			.records
			.iter()
			.map(|(input, _)| input.subject.as_str())
			.collect::<Vec<_>>(),
		vec!["reader", "delegate"]
	);
	for (input, decision) in &lease.records {
		assert_eq!(input.resource.attributes, json!({"owner":"saved-owner"}));
		assert_eq!(input.environment, lease.environment);
		assert!(decision.allowed);
		assert_eq!(decision.revision, 23);
	}
}
#[rstest]
#[case::first(0)]
#[case::last(1)]
#[tokio::test]
async fn one_denied_subject_denies_the_effect_without_omitting_other_audits(
	mut lease: Lease,
	#[case] denied: usize,
) {
	lease.subjects[denied] = "revoked".into();
	let resource = resource("tenant", &json!({}), "workspace", "1", json!({}));
	assert!(
		!decide(&mut lease, &resource, "workspace.read")
			.await
			.unwrap()
	);
	assert_eq!(lease.records.len(), 2);
	assert!(!lease.records[denied].1.allowed);
	assert!(lease.records[1 - denied].1.allowed);
}
#[rstest]
#[tokio::test]
async fn audit_failure_cannot_return_an_allowed_effect(mut lease: Lease) {
	lease.fail = true;
	let resource = resource("tenant", &json!({}), "workspace", "1", json!({}));
	let Error::Port(error) = decide(&mut lease, &resource, "workspace.read")
		.await
		.unwrap_err()
	else {
		panic!("expected opaque audit error");
	};
	assert!(error.is::<std::io::Error>());
	assert_eq!(lease.records.len(), 2);
}
#[rstest]
fn saved_resource_attributes_override_context_defaults_without_mutating_either_input() {
	let context = json!({"owner":"request-owner","created_by":"request-author","workspace_id":"current","nested":{"default":true}});
	let attributes = json!({"created_by":"saved-author","nested":{"saved":true}});
	let result = resource(
		"tenant",
		&context,
		"artifact",
		"saved-id",
		attributes.clone(),
	);
	assert_eq!(
		result.attributes,
		json!({"owner":"request-owner","created_by":"saved-author","workspace_id":"current","nested":{"saved":true}})
	);
	assert_eq!(result.id, "saved-id");
	assert_eq!(result.tenant, "tenant");
	assert_eq!(
		attributes,
		json!({"created_by":"saved-author","nested":{"saved":true}})
	);
	assert_eq!(context["created_by"], "request-author");
}
#[rstest]
#[case::null(Value::Null)]
#[case::array(json!([1,2]))]
#[case::text(json!("text"))]
fn non_object_attributes_retain_their_original_contract(#[case] attributes: Value) {
	assert_eq!(
		resource(
			"tenant",
			&json!({"owner":"context"}),
			"kind",
			"id",
			attributes.clone()
		)
		.attributes,
		attributes
	);
}
