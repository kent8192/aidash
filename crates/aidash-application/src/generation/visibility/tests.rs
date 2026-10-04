use super::*;
use aidash_domain::entities::{Task, TaskStatus};
use async_trait::async_trait;
use chrono::DateTime;
use rstest::{fixture, rstest};
use serde_json::Value;
use std::collections::BTreeSet;
use uuid::Uuid;

#[fixture]
fn job() -> Request {
	crate::generation::test_support::request(1)
}
struct Scope {
	inherited: bool,
	context: Value,
	missing_task: bool,
	task_visible: bool,
	denied: BTreeSet<&'static str>,
	workspace_failure: bool,
	calls: Vec<String>,
	decisions: Vec<Resource>,
}
#[fixture]
fn scope() -> Scope {
	Scope {
		inherited: false,
		context: json!({"stale":"previous workspace"}),
		missing_task: false,
		task_visible: true,
		denied: BTreeSet::new(),
		workspace_failure: false,
		calls: vec![],
		decisions: vec![],
	}
}
#[async_trait]
impl GenerationVisibility for Scope {
	fn inherited_lease(&self) -> bool {
		self.inherited
	}
	fn context(&mut self, value: Value) {
		self.calls.push("context".into());
		self.context = value;
	}
	fn resource(&self, kind: &str, id: &str, mut attributes: Value) -> Resource {
		for (key, value) in self.context.as_object().unwrap() {
			attributes
				.as_object_mut()
				.unwrap()
				.entry(key.clone())
				.or_insert_with(|| value.clone());
		}
		Resource {
			tenant: "tenant".into(),
			kind: kind.into(),
			id: id.into(),
			attributes,
		}
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.calls.push(format!("workspace:{id}"));
		if self.workspace_failure {
			return Err(crate::Error::External("authority unavailable".into()));
		}
		Ok(Resource {
			tenant: "tenant".into(),
			kind: "workspace".into(),
			id: id.to_string(),
			attributes: json!({"team":"research","policy_id":"untrusted context"}),
		})
	}
	async fn task(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Task>> {
		self.calls.push(format!("task:{id}:{workspace}"));
		Ok((!self.missing_task).then(|| Task {
			id,
			workspace_id: workspace,
			title: "Task".into(),
			description: String::new(),
			status: TaskStatus::Open,
			requirements: json!({}),
			owner: None,
			created_by: "alice".into(),
			dependencies: vec![],
			parent_id: None,
			revision: 1,
			created_at: DateTime::from_timestamp(1000, 0).unwrap(),
		}))
	}
	async fn task_visible(&mut self, task: &Task) -> Result<bool> {
		self.calls.push(format!("task_visible:{}", task.id));
		Ok(self.task_visible)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.calls.push(format!("decide:{action}:{}", resource.id));
		self.decisions.push(resource.clone());
		Ok(!self.denied.contains(action))
	}
}
#[rstest]
#[tokio::test]
async fn local_request_rechecks_task_and_workspace_before_generation_disclosure(
	job: Request,
	mut scope: Scope,
) {
	assert!(visible(&mut scope, &job).await.unwrap());
	assert_eq!(
		scope.calls,
		vec![
			"context".into(),
			format!("workspace:{}", job.workspace_id),
			format!("task:{}:{}", job.task_id, job.workspace_id),
			format!("task_visible:{}", job.task_id),
			"context".into(),
			format!("decide:workspace.read:{}", job.workspace_id),
			format!("decide:generation.read:{}", job.id)
		]
	);
	assert_eq!(
		scope.decisions[1].attributes,
		json!({"policy_id":"policy","root_subject":"alice","task_id":job.task_id,"team":"research"})
	);
	assert!(
		scope
			.decisions
			.iter()
			.all(|resource| resource.attributes.get("stale").is_none())
	);
}
#[rstest]
#[case("missing_task", false)]
#[case("task_visibility", false)]
#[case("workspace.read", true)]
#[case("generation.read", true)]
#[tokio::test]
async fn every_current_local_visibility_gate_can_block_a_saved_request(
	job: Request,
	mut scope: Scope,
	#[case] gate: &'static str,
	#[case] workspace_decided: bool,
) {
	match gate {
		"missing_task" => scope.missing_task = true,
		"task_visibility" => scope.task_visible = false,
		action => {
			scope.denied.insert(action);
		}
	}
	assert!(!visible(&mut scope, &job).await.unwrap());
	assert_eq!(
		scope
			.calls
			.iter()
			.any(|call| call.starts_with("decide:workspace.read:")),
		workspace_decided
	);
	if gate != "generation.read" {
		assert!(
			!scope
				.calls
				.iter()
				.any(|call| call.starts_with("decide:generation.read:"))
		);
	}
}
#[rstest]
#[case::task("task.read", 1)]
#[case::generation("generation.read", 2)]
#[tokio::test]
async fn foreign_request_requires_remote_task_and_generation_read_without_local_rows(
	mut job: Request,
	mut scope: Scope,
	#[case] denied: &'static str,
	#[case] decisions: usize,
) {
	job.home_node = "aidash://home".into();
	scope.denied.insert(denied);
	assert!(!visible(&mut scope, &job).await.unwrap());
	assert_eq!(scope.decisions.len(), decisions);
	assert_eq!(
		scope.decisions[0].id,
		format!("aidash://home/tasks/{}", job.task_id)
	);
	assert_eq!(scope.decisions[0].attributes, json!({}));
	assert!(
		!scope
			.calls
			.iter()
			.any(|call| call.starts_with("workspace:") || call.starts_with("task:"))
	);
}
#[rstest]
#[tokio::test]
async fn inherited_worker_context_survives_foreign_generation_visibility(
	mut job: Request,
	mut scope: Scope,
) {
	job.home_node = "aidash://home".into();
	scope.inherited = true;
	scope.context = json!({"workspace_id":job.workspace_id,"team":"leased"});
	assert!(visible(&mut scope, &job).await.unwrap());
	assert_eq!(
		scope.calls,
		vec![
			format!("decide:task.read:aidash://home/tasks/{}", job.task_id),
			format!("decide:generation.read:{}", job.id)
		]
	);
	assert_eq!(scope.decisions[1].attributes["team"], "leased");
}
#[rstest]
#[tokio::test]
async fn authority_lookup_failure_keeps_its_error_and_never_discloses_request(
	job: Request,
	mut scope: Scope,
) {
	scope.workspace_failure = true;
	assert!(
		matches!(visible(&mut scope,&job).await,Err(crate::Error::External(message)) if message=="authority unavailable")
	);
	assert!(scope.decisions.is_empty());
}
#[rstest]
fn portable_request_omits_internal_foreign_intent_and_credential_from_json(mut job: Request) {
	job.foreign_intent = Some(json!({"internal":"private"}));
	let json = serde_json::to_value(&job).unwrap();
	assert!(json.get("foreign_intent").is_none());
	assert!(json.get("credential_id").is_none());
	assert_eq!(json["policy_revision"], 7);
	assert_eq!(json["status"], "ACTIVE");
}

#[rstest]
#[case("missing_task")]
#[case("task_visibility")]
#[case("workspace.read")]
#[case("generation.read")]
#[tokio::test]
async fn native_local_reader_keeps_local_task_authority_even_with_a_home_marker(
	mut job: Request,
	mut scope: Scope,
	#[case] gate: &'static str,
) {
	job.home_node = "aidash://peer".into();
	match gate {
		"missing_task" => scope.missing_task = true,
		"task_visibility" => scope.task_visible = false,
		action => {
			scope.denied.insert(action);
		}
	}
	assert!(!local_visible(&mut scope, &job).await.unwrap());
	assert!(
		scope
			.calls
			.contains(&format!("workspace:{}", job.workspace_id))
	);
	assert!(
		scope
			.calls
			.contains(&format!("task:{}:{}", job.task_id, job.workspace_id))
	);
}
