use super::*;
use crate::{authorization::Snapshot, ports::authorization::lease::AuthorizationLease};
use aidash_domain::{
	RunMetadata,
	policy::{Decision, Evaluation},
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::Value;
struct Scope {
	snapshot: Snapshot,
	subjects: Vec<String>,
	environment: Value,
	context: Value,
	inherited: bool,
	owner: Option<String>,
	rows: Vec<(Uuid, String)>,
	run: Option<Run>,
	run_visible: bool,
	require_allowed: bool,
	calls: Vec<String>,
	owner_lookups: Vec<(Uuid, bool)>,
	records: Vec<(Evaluation, Decision)>,
	fail: Option<&'static str>,
}
fn run() -> Run {
	Run {
		id: Uuid::from_u128(9),
		task_id: Uuid::from_u128(10),
		workspace_id: Uuid::from_u128(1),
		home_node: "aidash://local".into(),
		agent_id: "agent".into(),
		agent_version: "1.0.0".into(),
		state_version: Default::default(),
		state: Default::default(),
		recovery: Default::default(),
		control: aidash_domain::RunControl::Active,
		context: Default::default(),
		step: 0,
		revision: 1,
		observed_input_seq: 0,
		ledger_worker_ready: true,
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: chrono::Utc::now(),
	}
}
#[fixture]
fn scope() -> Scope {
	Scope{snapshot:Snapshot{revision:31,bundle:serde_json::from_value(json!({"tenant":"tenant","subjects":{"reader":{"kind":"user"}},"policies":[{"id":"read","effect":"allow","subjects":{"any":true},"actions":["workspace.read"],"resources":{"kinds":["workspace"],"ids":[Uuid::from_u128(1),Uuid::from_u128(2),Uuid::from_u128(3)]}},{"id":"events","effect":"allow","subjects":{"any":true},"actions":["workspace.events"],"resources":{"kinds":["workspace"],"ids":[Uuid::from_u128(1),Uuid::from_u128(3)]}}]})).unwrap()},subjects:vec!["reader".into()],environment:json!({"transport":"worker"}),context:json!({"workspace_id":Uuid::from_u128(1),"created_by":"lease-default"}),inherited:false,owner:Some("stored-owner".into()),rows:vec![(Uuid::from_u128(3),"third-owner".into()),(Uuid::from_u128(2),"second-owner".into()),(Uuid::from_u128(4),"fourth-owner".into()),(Uuid::from_u128(1),"first-owner".into())],run:Some(run()),run_visible:true,require_allowed:true,calls:vec![],owner_lookups:vec![],records:vec![],fail:None}
}
impl Scope {
	fn touch(&mut self, name: &'static str) -> Result<()> {
		self.calls.push(name.into());
		if self.fail == Some(name) {
			Err(Error::Port(Box::new(std::io::Error::other(name))))
		} else {
			Ok(())
		}
	}
}
#[async_trait]
impl AuthorizationLease for Scope {
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
		self.touch("audit")?;
		self.records.extend_from_slice(records);
		Ok(())
	}
}
#[async_trait]
impl WorkspaceResourceScope for Scope {
	fn inherited(&self) -> bool {
		self.inherited
	}
	fn context(&self) -> &Value {
		&self.context
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		super::super::lease::resource("tenant", &self.context, kind, id, attributes)
	}
	async fn owner(&mut self, id: Uuid, lock: bool) -> Result<Option<String>> {
		self.touch("owner")?;
		self.owner_lookups.push((id, lock));
		Ok(self.owner.clone())
	}
}
#[async_trait]
impl WorkspaceAuthorityScope for Scope {
	async fn locked_owner(&mut self, id: Uuid) -> Result<Option<String>> {
		self.owner(id, true).await
	}
	fn identity(&self) -> (&str, &str) {
		("tenant", "reader")
	}
	async fn all_owners(&mut self) -> Result<Vec<(Uuid, String)>> {
		self.touch("all_owners")?;
		Ok(self.rows.clone())
	}
	async fn event_owners(&mut self, selected: Option<Uuid>) -> Result<Vec<(Uuid, String)>> {
		self.touch("event_owners")?;
		Ok(self
			.rows
			.iter()
			.filter(|(id, _)| selected.is_none_or(|selected| selected == *id))
			.cloned()
			.collect())
	}
}
#[async_trait]
impl RunInteractionScope for Scope {
	async fn run(&mut self, id: Uuid) -> Result<Option<Run>> {
		assert_eq!(id, Uuid::from_u128(9));
		self.touch("run")?;
		Ok(self.run.clone())
	}
	fn set_context(&mut self, context: Value) {
		self.calls.push("context".into());
		self.context = context;
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		assert_eq!(action, "workspace.read");
		assert_eq!(resource.attributes["owner"], "stored-owner");
		self.touch("require")?;
		if self.require_allowed {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}
	async fn run_visible(&mut self, run: &RunMetadata) -> Result<bool> {
		assert_eq!(run.id, Uuid::from_u128(9));
		assert_eq!(self.context["owner"], "stored-owner");
		self.touch("run_visible")?;
		Ok(self.run_visible)
	}
}
#[rstest]
#[case::ordinary(false, true)]
#[case::inherited(true, false)]
#[tokio::test]
async fn workspace_resource_retains_stored_owner_context_and_lock_mode(
	mut scope: Scope,
	#[case] inherited: bool,
	#[case] lock: bool,
) {
	scope.inherited = inherited;
	let id = Uuid::from_u128(1);
	let resource = resource(&mut scope, id).await.unwrap();
	assert_eq!(
		resource.attributes,
		json!({"owner":"stored-owner","workspace_id":id,"created_by":"lease-default"})
	);
	assert_eq!(scope.owner_lookups, vec![(id, lock)]);
}
#[rstest]
#[case::different(json!({"workspace_id":Uuid::from_u128(2)}))]
#[case::missing(json!({}))]
#[case::malformed(json!({"workspace_id":1}))]
#[tokio::test]
async fn inherited_resource_cannot_expand_its_workspace_before_any_lookup(
	mut scope: Scope,
	#[case] context: Value,
) {
	scope.inherited = true;
	scope.context = context;
	assert!(matches!(
		resource(&mut scope, Uuid::from_u128(1)).await,
		Err(Error::Forbidden)
	));
	assert!(scope.calls.is_empty());
}
#[rstest]
#[tokio::test]
async fn missing_workspace_resource_is_a_denial(mut scope: Scope) {
	scope.owner = None;
	assert!(matches!(
		resource(&mut scope, Uuid::from_u128(1)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, vec!["owner"]);
}
#[rstest]
#[tokio::test]
async fn missing_ownership_overrides_allow_and_keeps_the_unavailable_audit(mut scope: Scope) {
	scope.owner = None;
	assert!(
		!allowed(&mut scope, Uuid::from_u128(1), "workspace.read")
			.await
			.unwrap()
	);
	assert_eq!(scope.owner_lookups, vec![(Uuid::from_u128(1), true)]);
	assert_eq!(scope.calls, vec!["owner", "audit"]);
	let (input, decision) = &scope.records[0];
	assert_eq!(input.resource.attributes, json!({}));
	assert_eq!(decision.reason, "resource_unavailable");
	assert_eq!(decision.revision, 31);
	assert_eq!(decision.matched_policies, vec!["read"]);
	assert!(!decision.allowed);
}
#[rstest]
#[tokio::test]
async fn ownership_decision_uses_locked_identity_without_context_attribute_defaults(
	mut scope: Scope,
) {
	assert!(
		allowed(&mut scope, Uuid::from_u128(1), "workspace.read")
			.await
			.unwrap()
	);
	let (input, decision) = &scope.records[0];
	assert_eq!(input.subject, "reader");
	assert_eq!(input.environment, json!({"transport":"worker"}));
	assert_eq!(
		input.resource.attributes,
		json!({"owner":"stored-owner","workspace_id":Uuid::from_u128(1)})
	);
	assert!(decision.allowed);
	assert_eq!(decision.revision, 31);
}
#[rstest]
#[tokio::test]
async fn visible_workspaces_keep_repository_order_and_audit_each_row(mut scope: Scope) {
	assert_eq!(
		visible(&mut scope, "workspace.read").await.unwrap(),
		vec![Uuid::from_u128(3), Uuid::from_u128(2), Uuid::from_u128(1)]
	);
	assert_eq!(scope.records.len(), 4);
	assert_eq!(
		scope
			.records
			.iter()
			.map(|(input, _)| input.resource.attributes["owner"].as_str().unwrap())
			.collect::<Vec<_>>(),
		vec!["third-owner", "second-owner", "fourth-owner", "first-owner"]
	);
}
#[rstest]
#[tokio::test]
async fn event_workspaces_require_read_then_events_without_refetching_owners(mut scope: Scope) {
	assert_eq!(
		event_workspaces(&mut scope, None).await.unwrap(),
		vec![Uuid::from_u128(3), Uuid::from_u128(1)]
	);
	assert!(scope.owner_lookups.is_empty());
	assert_eq!(
		scope
			.records
			.iter()
			.map(|(input, _)| (input.resource.id.clone(), input.action.clone()))
			.collect::<Vec<_>>(),
		vec![
			(Uuid::from_u128(3).to_string(), "workspace.read".into()),
			(Uuid::from_u128(3).to_string(), "workspace.events".into()),
			(Uuid::from_u128(2).to_string(), "workspace.read".into()),
			(Uuid::from_u128(2).to_string(), "workspace.events".into()),
			(Uuid::from_u128(4).to_string(), "workspace.read".into()),
			(Uuid::from_u128(1).to_string(), "workspace.read".into()),
			(Uuid::from_u128(1).to_string(), "workspace.events".into())
		]
	);
}
#[rstest]
#[tokio::test]
async fn selected_missing_workspace_records_unavailability_without_disclosing_rows(
	mut scope: Scope,
) {
	scope.rows.clear();
	assert!(
		event_workspaces(&mut scope, Some(Uuid::from_u128(1)))
			.await
			.unwrap()
			.is_empty()
	);
	assert_eq!(scope.calls, vec!["event_owners", "audit"]);
	assert_eq!(scope.records[0].1.reason, "resource_unavailable");
	assert_eq!(scope.records[0].0.resource.attributes, json!({}));
}
#[rstest]
#[tokio::test]
async fn workspace_requirement_keeps_the_public_denial_after_auditing(mut scope: Scope) {
	assert!(matches!(
		require(&mut scope, Uuid::from_u128(4), "workspace.read").await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, vec!["owner", "audit"]);
	assert!(!scope.records[0].1.allowed);
}
#[rstest]
#[tokio::test]
async fn interaction_sets_saved_workspace_context_before_read_policy_and_run_provenance(
	mut scope: Scope,
) {
	let result = run_for_interaction(&mut scope, Uuid::from_u128(9))
		.await
		.unwrap();
	assert_eq!(result.id, Uuid::from_u128(9));
	assert_eq!(
		scope.calls,
		vec!["run", "owner", "context", "require", "run_visible"]
	);
	assert_eq!(scope.context["owner"], "stored-owner");
}
#[rstest]
#[case::run(0,vec!["run"])]
#[case::workspace(1,vec!["run","owner"])]
#[case::read_policy(2,vec!["run","owner","context","require"])]
#[case::provenance(3,vec!["run","owner","context","require","run_visible"])]
#[tokio::test]
async fn interaction_denial_stops_at_the_original_dependency(
	mut scope: Scope,
	#[case] boundary: u8,
	#[case] calls: Vec<&str>,
) {
	match boundary {
		0 => scope.run = None,
		1 => scope.owner = None,
		2 => scope.require_allowed = false,
		_ => scope.run_visible = false,
	};
	assert!(matches!(
		run_for_interaction(&mut scope, Uuid::from_u128(9)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, calls);
}
#[rstest]
#[case::owner("owner")]
#[case::audit("audit")]
#[tokio::test]
async fn ownership_errors_keep_their_opaque_identity(mut scope: Scope, #[case] fail: &'static str) {
	scope.fail = Some(fail);
	let Error::Port(error) = allowed(&mut scope, Uuid::from_u128(1), "workspace.read")
		.await
		.unwrap_err()
	else {
		panic!("expected opaque adapter error");
	};
	assert!(error.is::<std::io::Error>());
	assert_eq!(scope.calls.last().unwrap(), fail);
}
#[rstest]
#[case::run("run")]
#[case::owner("owner")]
#[case::require("require")]
#[case::provenance("run_visible")]
#[tokio::test]
async fn interaction_errors_keep_their_opaque_identity(
	mut scope: Scope,
	#[case] fail: &'static str,
) {
	scope.fail = Some(fail);
	let Error::Port(error) = run_for_interaction(&mut scope, Uuid::from_u128(9))
		.await
		.unwrap_err()
	else {
		panic!("expected opaque adapter error");
	};
	assert!(error.is::<std::io::Error>());
	assert_eq!(scope.calls.last().unwrap(), fail);
}
