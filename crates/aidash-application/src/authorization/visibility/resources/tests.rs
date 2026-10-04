use super::*;
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::Value;
use std::collections::BTreeMap;

#[fixture]
fn task() -> Task {
	Task {
		id: Uuid::from_u128(1),
		workspace_id: Uuid::from_u128(2),
		title: "Task".into(),
		description: String::new(),
		status: aidash_domain::TaskStatus::Open,
		requirements: json!({}),
		owner: None,
		created_by: "stored-task-author".into(),
		dependencies: vec![],
		parent_id: None,
		revision: 0,
		created_at: chrono::Utc::now(),
	}
}
#[fixture]
fn artifact() -> Artifact {
	Artifact {
		id: Uuid::from_u128(3),
		workspace_id: Uuid::from_u128(2),
		task_id: Uuid::from_u128(1),
		kind: "text".into(),
		name: "Artifact".into(),
		content: json!({}),
		created_by: "stored-artifact-author".into(),
		idempotency_key: "key".into(),
		created_at: chrono::Utc::now(),
	}
}
fn message(id: u128, sender: &str) -> Message {
	Message {
		id: Uuid::from_u128(id),
		workspace_id: Uuid::from_u128(2),
		sender: sender.into(),
		content: "same content".into(),
		idempotency_key: None,
		created_at: chrono::Utc::now(),
	}
}
fn human(id: u128) -> HumanRequest {
	HumanRequest {
		id: Uuid::from_u128(id),
		workspace_id: Uuid::from_u128(2),
		run_id: Uuid::from_u128(9),
		kind: "approval".into(),
		prompt: String::new(),
		response: None,
		answered_by: None,
		created_at: chrono::Utc::now(),
	}
}
struct Scope {
	task: Option<Task>,
	artifact: Option<Artifact>,
	messages: Vec<Message>,
	humans: Vec<HumanRequest>,
	cache: BTreeMap<Uuid, bool>,
	calls: Vec<String>,
	decisions: Vec<Resource>,
	denied: Option<&'static str>,
	denied_id: Option<Uuid>,
	fail: Option<&'static str>,
	output: bool,
	lookups: Vec<(String, Uuid, Option<Uuid>)>,
	legacy: Option<(Option<Uuid>, Option<String>, Option<String>)>,
}
#[fixture]
fn scope() -> Scope {
	Scope {
		task: Some(task()),
		artifact: Some(artifact()),
		messages: vec![message(4, "stored-sender")],
		humans: vec![],
		cache: BTreeMap::new(),
		calls: vec![],
		decisions: vec![],
		denied: None,
		denied_id: None,
		fail: None,
		output: true,
		lookups: vec![],
		legacy: None,
	}
}
impl Scope {
	fn touch(&mut self, name: &'static str) -> Result<()> {
		self.calls.push(name.into());
		if self.fail == Some(name) {
			return Err(Error::Port(Box::new(std::io::Error::other(
				"resource adapter fault",
			))));
		}
		Ok(())
	}
}
#[async_trait]
impl ResourceVisibilityScope for Scope {
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		Resource {
			tenant: "tenant".into(),
			kind: kind.into(),
			id: id.into(),
			attributes,
		}
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.touch("workspace")?;
		Ok(self.resource(
			"workspace",
			&id.to_string(),
			json!({"workspace_id":id,"owner":"stored-owner"}),
		))
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		let name = match action {
			"task.read" => "task.read",
			"artifact.read" => "artifact.read",
			"message.read" => "message.read",
			"human.read" => "human.read",
			_ => panic!("unexpected policy action"),
		};
		self.touch(name)?;
		self.decisions.push(resource.clone());
		Ok(self.denied != Some(name)
			&& self.denied_id.map(|id| id.to_string()) != Some(resource.id.clone()))
	}
	async fn artifact_task(&mut self, a: &Artifact) -> Result<Option<Task>> {
		self.touch("artifact_task")?;
		self.lookups
			.push(("artifact_task".into(), a.task_id, Some(a.workspace_id)));
		Ok(self.task.clone())
	}
	async fn output_visible(&mut self, workspace: Uuid, kind: &str, id: Uuid) -> Result<bool> {
		self.touch("output")?;
		self.calls.push(format!("output:{workspace}:{kind}:{id}"));
		Ok(self.output)
	}
	fn cached_human(&self, id: Uuid) -> Option<bool> {
		self.cache.get(&id).copied()
	}
	fn remember_human(&mut self, id: Uuid, allowed: bool) {
		self.cache.insert(id, allowed);
	}
	async fn humans(&mut self, workspace: Uuid, run: Uuid) -> Result<Vec<HumanRequest>> {
		assert_eq!(workspace, Uuid::from_u128(2));
		assert_eq!(run, Uuid::from_u128(9));
		self.touch("humans")?;
		Ok(self.humans.clone())
	}
}
#[async_trait]
impl ResourceAccessScope for Scope {
	async fn task_any(&mut self, id: Uuid) -> Result<Option<Task>> {
		self.touch("task_any")?;
		Ok(self.task.clone().map(|mut task| {
			task.id = id;
			task
		}))
	}
}
#[async_trait]
impl ResourceEventScope for Scope {
	async fn event_task(&mut self, id: Uuid, workspace: Option<Uuid>) -> Result<Option<Task>> {
		self.touch("event_task")?;
		self.lookups.push(("task".into(), id, workspace));
		Ok(self.task.clone())
	}
	async fn artifact(&mut self, id: Uuid, workspace: Option<Uuid>) -> Result<Option<Artifact>> {
		self.touch("artifact")?;
		self.lookups.push(("artifact".into(), id, workspace));
		Ok(self.artifact.clone())
	}
	async fn messages_id(&mut self, id: Uuid, workspace: Option<Uuid>) -> Result<Vec<Message>> {
		self.touch("messages_id")?;
		self.lookups.push(("message".into(), id, workspace));
		Ok(self.messages.clone())
	}
	async fn messages_legacy(
		&mut self,
		workspace: Option<Uuid>,
		sender: Option<&str>,
		content: Option<&str>,
	) -> Result<Vec<Message>> {
		self.touch("messages_legacy")?;
		self.legacy = Some((
			workspace,
			sender.map(str::to_owned),
			content.map(str::to_owned),
		));
		Ok(self.messages.clone())
	}
}
fn event(kind: &str, data: Value) -> Event {
	Event {
		sequence: 1,
		id: Uuid::from_u128(10),
		node_id: "aidash://node".into(),
		workspace_id: Some(Uuid::from_u128(2)),
		kind: kind.into(),
		data,
		created_at: chrono::Utc::now(),
	}
}

#[rstest]
#[tokio::test]
async fn artifact_disclosure_rechecks_saved_author_parent_task_and_both_output_journals(
	mut scope: Scope,
	artifact: Artifact,
) {
	assert!(artifact_visible(&mut scope, &artifact).await.unwrap());
	assert_eq!(
		scope.decisions[0].attributes,
		json!({"workspace_id":artifact.workspace_id,"owner":"stored-owner","created_by":"stored-artifact-author","task_id":artifact.task_id,"kind":"text"})
	);
	assert_eq!(
		scope.decisions[1].attributes["created_by"],
		json!("stored-task-author")
	);
	assert_eq!(
		scope.lookups,
		vec![(
			"artifact_task".into(),
			artifact.task_id,
			Some(artifact.workspace_id)
		)]
	);
	assert_eq!(
		scope
			.calls
			.iter()
			.filter(|c| c.as_str() == "output")
			.count(),
		2
	);
}

#[rstest]
#[case("artifact.read")]
#[case("missing_task")]
#[case("task.read")]
#[case("output")]
#[tokio::test]
async fn artifact_visibility_never_bypasses_a_hidden_parent_or_producer(
	mut scope: Scope,
	artifact: Artifact,
	#[case] gate: &'static str,
) {
	match gate {
		"missing_task" => scope.task = None,
		"output" => scope.output = false,
		action => scope.denied = Some(action),
	}
	assert!(!artifact_visible(&mut scope, &artifact).await.unwrap());
	if gate == "artifact.read" {
		assert!(!scope.calls.contains(&"artifact_task".into()));
	}
	if matches!(gate, "artifact.read" | "missing_task" | "task.read") {
		assert!(!scope.calls.contains(&format!(
			"output:{}:artifact:{}",
			artifact.workspace_id, artifact.id
		)));
	}
}

#[rstest]
#[tokio::test]
async fn message_authorship_and_sender_are_taken_from_the_saved_row(mut scope: Scope) {
	let m = message(4, "stored-sender");
	assert!(message_visible(&mut scope, &m).await.unwrap());
	assert_eq!(
		scope.decisions[0].attributes,
		json!({"workspace_id":m.workspace_id,"owner":"stored-owner","created_by":"stored-sender","sender":"stored-sender"})
	);
}

#[rstest]
#[case("message.read")]
#[case("output")]
#[tokio::test]
async fn messages_require_both_current_read_policy_and_source_visibility(
	mut scope: Scope,
	#[case] gate: &'static str,
) {
	if gate == "output" {
		scope.output = false;
	} else {
		scope.denied = Some(gate);
	}
	assert!(
		!message_visible(&mut scope, &message(4, "sender"))
			.await
			.unwrap()
	);
	if gate == "message.read" {
		assert!(!scope.calls.contains(&"output".into()));
	}
}

#[rstest]
#[tokio::test]
async fn replay_rechecks_new_human_membership_while_reusing_immutable_request_decisions(
	mut scope: Scope,
) {
	scope.humans = vec![human(5)];
	assert!(
		human_reads(&mut scope, Uuid::from_u128(2), Uuid::from_u128(9))
			.await
			.unwrap()
	);
	scope.humans.push(human(6));
	scope.denied_id = Some(Uuid::from_u128(6));
	assert!(
		!human_reads(&mut scope, Uuid::from_u128(2), Uuid::from_u128(9))
			.await
			.unwrap()
	);
	assert_eq!(
		scope
			.calls
			.iter()
			.filter(|c| c.as_str() == "humans")
			.count(),
		2
	);
	assert_eq!(
		scope
			.calls
			.iter()
			.filter(|c| c.as_str() == "human.read")
			.count(),
		2
	);
	assert_eq!(scope.cache.get(&Uuid::from_u128(5)), Some(&true));
	assert_eq!(scope.cache.get(&Uuid::from_u128(6)), Some(&false));
}

#[rstest]
#[case(true)]
#[case(false)]
#[tokio::test]
async fn cached_human_allow_and_deny_decisions_skip_only_immutable_attribute_reads(
	mut scope: Scope,
	#[case] allowed: bool,
) {
	scope.cache.insert(Uuid::from_u128(5), allowed);
	assert_eq!(human_visible(&mut scope, &human(5)).await.unwrap(), allowed);
	assert!(scope.calls.is_empty());
}

#[rstest]
#[case("missing")]
#[case("policy")]
#[case("output")]
#[tokio::test]
async fn direct_task_reads_hide_missing_denied_and_tainted_tasks(
	mut scope: Scope,
	#[case] gate: &str,
) {
	match gate {
		"missing" => scope.task = None,
		"policy" => scope.denied = Some("task.read"),
		"output" => scope.output = false,
		_ => panic!("unknown case"),
	}
	assert!(matches!(
		task_read(&mut scope, Uuid::from_u128(1)).await,
		Err(Error::Forbidden)
	));
}

#[rstest]
#[tokio::test]
async fn related_tasks_must_be_readable_and_in_the_same_workspace(mut scope: Scope) {
	let input = NewTask {
		title: String::new(),
		description: String::new(),
		requirements: json!({}),
		dependencies: vec![Uuid::from_u128(1)],
		parent_id: Some(Uuid::from_u128(7)),
	};
	assert!(matches!(
		related_tasks(&mut scope, Uuid::from_u128(99), &input).await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		scope
			.calls
			.iter()
			.filter(|c| c.as_str() == "task_any")
			.count(),
		1
	);
}

#[rstest]
#[tokio::test]
async fn artifact_creation_authorizes_parent_before_using_its_scope(mut scope: Scope) {
	let resource = artifact_creation_resource(&mut scope, Uuid::from_u128(1), "new-author")
		.await
		.unwrap();
	assert_eq!(resource.kind, "artifact");
	assert_eq!(resource.id, Uuid::from_u128(1).to_string());
	assert_eq!(resource.attributes["created_by"], json!("new-author"));
	assert_eq!(
		scope.decisions[0].attributes["created_by"],
		json!("stored-task-author")
	);
}

#[rstest]
#[case(json!({"task":{"id":Uuid::from_u128(1)},"task_id":Uuid::from_u128(99),"id":Uuid::from_u128(98)}),Uuid::from_u128(1))]
#[case(json!({"task":{"id":"bad"},"task_id":Uuid::from_u128(1),"id":Uuid::from_u128(98)}),Uuid::from_u128(1))]
#[case(json!({"task_id":null,"id":Uuid::from_u128(1)}),Uuid::from_u128(1))]
#[tokio::test]
async fn task_event_identifier_precedence_preserves_legacy_envelopes(
	mut scope: Scope,
	#[case] data: Value,
	#[case] id: Uuid,
) {
	assert_eq!(
		event_visible(&mut scope, &event("task.updated", data))
			.await
			.unwrap(),
		Some(true)
	);
	assert_eq!(
		scope.lookups[0],
		("task".into(), id, Some(Uuid::from_u128(2)))
	);
}

#[rstest]
#[tokio::test]
async fn task_artifact_events_require_both_resources_even_if_event_claims_an_allowed_author(
	mut scope: Scope,
) {
	scope.denied = Some("artifact.read");
	let data = json!({"task":{"id":Uuid::from_u128(1)},"artifact":{"id":Uuid::from_u128(3),"created_by":"forged"}});
	assert_eq!(
		event_visible(&mut scope, &event("task.updated", data))
			.await
			.unwrap(),
		Some(false)
	);
	assert_eq!(
		scope.decisions[1].attributes["created_by"],
		json!("stored-artifact-author")
	);
}

#[rstest]
#[case("task.created",json!({}))]
#[case("artifact.created",json!({"id":"bad"}))]
#[case("message.thread_opened",json!({"sender":"x","content":"x"}))]
#[case("message.thread_opened",json!({"id":false}))]
#[tokio::test]
async fn missing_or_malformed_required_event_ids_fail_closed_without_loading_records(
	mut scope: Scope,
	#[case] kind: &str,
	#[case] data: Value,
) {
	assert_eq!(
		event_visible(&mut scope, &event(kind, data)).await.unwrap(),
		Some(false)
	);
	assert!(scope.calls.is_empty());
}

#[rstest]
#[case("human.created")]
#[case("run.created")]
#[case("message.updated")]
#[case("new.family")]
#[tokio::test]
async fn unrelated_event_families_are_left_for_their_registered_reader(
	mut scope: Scope,
	#[case] kind: &str,
) {
	assert_eq!(
		event_visible(&mut scope, &event(kind, json!({})))
			.await
			.unwrap(),
		None
	);
	assert!(scope.calls.is_empty());
}

#[rstest]
#[case(4)]
#[case(5)]
#[tokio::test]
async fn ambiguous_legacy_message_events_authorize_every_saved_match(
	mut scope: Scope,
	#[case] denied: u128,
) {
	scope.messages = vec![message(4, "sender"), message(5, "sender")];
	scope.denied_id = Some(Uuid::from_u128(denied));
	assert_eq!(
		event_visible(
			&mut scope,
			&event(
				"message.created",
				json!({"sender":"sender","content":"same content"})
			)
		)
		.await
		.unwrap(),
		Some(false)
	);
	assert_eq!(
		scope.legacy,
		Some((
			Some(Uuid::from_u128(2)),
			Some("sender".into()),
			Some("same content".into())
		))
	);
	assert!(!scope.calls.contains(&"messages_id".into()));
}

#[rstest]
#[tokio::test]
async fn explicit_message_event_ids_do_not_fall_back_to_same_content_rows(mut scope: Scope) {
	scope.messages.clear();
	assert_eq!(
		event_visible(
			&mut scope,
			&event(
				"message.created",
				json!({"id":Uuid::from_u128(90),"sender":"sender","content":"same content"})
			)
		)
		.await
		.unwrap(),
		Some(false)
	);
	assert_eq!(
		scope.lookups,
		vec![(
			"message".into(),
			Uuid::from_u128(90),
			Some(Uuid::from_u128(2))
		)]
	);
	assert!(scope.legacy.is_none());
}

#[rstest]
#[case("workspace")]
#[case("artifact.read")]
#[case("artifact_task")]
#[case("task.read")]
#[case("output")]
#[tokio::test]
async fn resource_adapter_errors_are_preserved_instead_of_becoming_allow_or_deny(
	mut scope: Scope,
	artifact: Artifact,
	#[case] boundary: &'static str,
) {
	scope.fail = Some(boundary);
	let error = artifact_visible(&mut scope, &artifact).await.unwrap_err();
	assert!(matches!(error,Error::Port(ref e) if e.to_string()=="resource adapter fault"));
	assert_eq!(scope.calls.last().map(String::as_str), Some(boundary));
}

#[rstest]
#[case("event_task", "task.created")]
#[case("artifact", "artifact.created")]
#[case("messages_id", "message.created")]
#[case("messages_legacy", "message.created")]
#[tokio::test]
async fn event_membership_errors_stop_disclosure(
	mut scope: Scope,
	#[case] boundary: &'static str,
	#[case] kind: &str,
) {
	scope.fail = Some(boundary);
	let data = if boundary == "messages_legacy" {
		json!({"sender":"sender","content":"same content"})
	} else {
		json!({"id":Uuid::from_u128(1)})
	};
	let error = event_visible(&mut scope, &event(kind, data))
		.await
		.unwrap_err();
	assert!(matches!(error,Error::Port(ref e) if e.to_string()=="resource adapter fault"));
	assert_eq!(scope.calls.last().map(String::as_str), Some(boundary));
}
