use super::*;
use crate::Error;
use aidash_domain::{TaskStatus, policy::Resource};
use async_trait::async_trait;
use chrono::Utc;
use rstest::{fixture, rstest};
use serde_json::Value;
struct Scope {
	tenant: String,
	subject: String,
	thread: Thread,
	task: Task,
	calls: Vec<String>,
	resources: Vec<(Resource, String)>,
	task_keys: Vec<Option<String>>,
	task_inputs: Vec<(Value, String)>,
	messages: Vec<(String, String, String)>,
	title_writes: Vec<(i64, String)>,
	denied: Option<&'static str>,
	fail: Option<&'static str>,
}
#[fixture]
fn scope() -> Scope {
	let now = Utc::now();
	Scope {
		tenant: "tenant".into(),
		subject: "primary".into(),
		thread: Thread {
			scratch_space_id: uuid::Uuid::nil(),
			node: "brave-otter-k3m9q2xa".parse().unwrap(),
			id: Uuid::from_u128(1),
			title: "saved".into(),
			created_by: "subject".into(),
			revision: 1,
			created_at: now,
		},
		task: Task {
			id: Uuid::from_u128(2),
			thread_id: Uuid::from_u128(1),
			title: "task".into(),
			description: "task".into(),
			status: TaskStatus::Open,
			goal: None,
			requirements: json!({}),
			owner: None,
			created_by: "primary".into(),
			dependencies: vec![],
			parent_id: None,
			started_by_duty: None,
			revision: 1,
			created_at: now,
		},
		calls: vec![],
		resources: vec![],
		task_keys: vec![],
		task_inputs: vec![],
		messages: vec![],
		title_writes: vec![],
		denied: None,
		fail: None,
	}
}
fn input() -> NewTask {
	NewTask {
		title: "task".into(),
		description: "日本語".into(),
		goal: None,
		requirements: json!({"kind":"model"}),
		dependencies: vec![Uuid::from_u128(7)],
		parent_id: Some(Uuid::from_u128(8)),
	}
}
impl Scope {
	fn touch(&mut self, call: &'static str) -> Result<()> {
		self.calls.push(call.into());
		if self.denied == Some(call) {
			return Err(Error::Forbidden);
		}
		if self.fail == Some(call) {
			return Err(Error::Port(Box::new(std::io::Error::other(call))));
		}
		Ok(())
	}
}
#[async_trait]
impl ThreadMutations for Scope {
	fn identity(&self) -> (&str, &str) {
		(&self.tenant, &self.subject)
	}
	fn resource(&self, kind: &str, id: Uuid, attributes: Value) -> Resource {
		Resource {
			tenant: self.tenant.clone(),
			kind: kind.into(),
			id: id.to_string(),
			attributes,
		}
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.resources.push((resource.clone(), action.into()));
		let action = match action {
			"thread.create" => "thread.create",
			"task.read" => "task.read",
			_ => panic!("unexpected resource action"),
		};
		self.touch(action)
	}
	async fn require_thread(&mut self, id: Uuid, action: &str) -> Result<()> {
		assert_eq!(id, self.thread.id);
		let action = match action {
			"thread.read" => "thread.read",
			"thread.update" => "thread.update",
			"task.create" => "task.create",
			"message.create" => "message.create",
			_ => panic!("unexpected thread action"),
		};
		self.touch(action)
	}
	async fn use_node(
		&mut self,
		requested: Option<&aidash_domain::nodes::NodeName>,
	) -> Result<aidash_domain::nodes::NodeName> {
		self.touch("node.use")?;
		Ok(requested
			.cloned()
			.unwrap_or_else(|| "brave-otter-k3m9q2xa".parse().unwrap()))
	}
	async fn insert_thread(
		&mut self,
		id: Uuid,
		_: &aidash_domain::nodes::NodeName,
		title: &str,
	) -> Result<Thread> {
		assert_eq!(id, self.thread.id);
		assert_eq!(title, "title");
		self.touch("thread_insert")?;
		Ok(self.thread.clone())
	}
	async fn record_owner(&mut self, id: Uuid) -> Result<()> {
		assert_eq!(id, self.thread.id);
		self.touch("owner_record")
	}
	async fn update_title(&mut self, id: Uuid, revision: i64, title: &str) -> Result<Thread> {
		assert_eq!(id, self.thread.id);
		self.touch("title_update")?;
		self.title_writes.push((revision, title.to_owned()));
		Ok(self.thread.clone())
	}
	async fn related_tasks(&mut self, id: Uuid, input: &NewTask) -> Result<()> {
		assert_eq!(id, self.thread.id);
		assert_eq!(input.dependencies, vec![Uuid::from_u128(7)]);
		assert_eq!(input.parent_id, Some(Uuid::from_u128(8)));
		self.touch("related_tasks")
	}
	async fn insert_task(
		&mut self,
		id: Uuid,
		input: &NewTask,
		created_by: &str,
		key: Option<&str>,
	) -> Result<Task> {
		assert_eq!(id, self.thread.id);
		self.touch("task_insert")?;
		self.task_keys.push(key.map(str::to_owned));
		self.task_inputs.push((json!(input), created_by.into()));
		Ok(self.task.clone())
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		assert_eq!(task.id, self.task.id);
		self.touch("task_resource")?;
		Ok(self.resource(
			"task",
			task.id,
			json!({"created_by":task.created_by,"thread_id":task.thread_id}),
		))
	}
	async fn insert_message(
		&mut self,
		id: Uuid,
		sender: &str,
		content: &str,
		key: &str,
	) -> Result<()> {
		assert_eq!(id, self.thread.id);
		self.touch("message_insert")?;
		self.messages
			.push((sender.into(), content.into(), key.into()));
		Ok(())
	}
}
#[rstest]
#[tokio::test]
async fn thread_creation_authorizes_the_saved_primary_owner_before_atomic_writes(mut scope: Scope) {
	let value = create(&mut scope, Uuid::from_u128(1), "title", None)
		.await
		.unwrap();
	assert_eq!(value.id, scope.thread.id);
	assert_eq!(
		scope.calls,
		vec!["thread.create", "node.use", "thread_insert", "owner_record"]
	);
	assert_eq!(
		scope.resources[0].0.attributes,
		json!({"owner":"primary","thread_id":Uuid::from_u128(1)})
	);
	assert_eq!(scope.resources[0].0.tenant, "tenant");
}
#[rstest]
#[tokio::test]
async fn thread_creation_denial_prevents_the_thread_and_ownership_writes(mut scope: Scope) {
	scope.denied = Some("thread.create");
	assert!(matches!(
		create(&mut scope, Uuid::from_u128(1), "title", None).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, vec!["thread.create"]);
}
#[rstest]
#[case::policy("thread.create")]
#[case::node("node.use")]
#[case::row("thread_insert")]
#[case::ownership("owner_record")]
#[tokio::test]
async fn creation_errors_preserve_their_identity_at_the_original_transaction_boundary(
	mut scope: Scope,
	#[case] fail: &'static str,
) {
	scope.fail = Some(fail);
	let Error::Port(error) = create(&mut scope, Uuid::from_u128(1), "title", None)
		.await
		.unwrap_err()
	else {
		panic!("expected opaque error");
	};
	assert!(error.is::<std::io::Error>());
	assert_eq!(scope.calls.last().unwrap(), fail);
}
#[rstest]
#[tokio::test]
async fn title_update_requires_read_before_update_and_keeps_revision_and_text(mut scope: Scope) {
	let title = "日本語の題名";
	let value = update(&mut scope, Uuid::from_u128(1), 19, title)
		.await
		.unwrap();
	assert_eq!(value.id, scope.thread.id);
	assert_eq!(
		scope.calls,
		vec!["thread.read", "thread.update", "title_update"]
	);
	assert_eq!(scope.title_writes, vec![(19, title.to_owned())]);
}
#[rstest]
#[case::read("thread.read")]
#[case::update("thread.update")]
#[tokio::test]
async fn title_update_denial_prevents_the_atomic_write(
	mut scope: Scope,
	#[case] action: &'static str,
) {
	scope.denied = Some(action);
	assert!(matches!(
		update(&mut scope, Uuid::from_u128(1), 1, "Renamed").await,
		Err(Error::Forbidden)
	));
	assert!(scope.title_writes.is_empty());
	assert_eq!(scope.calls.last().unwrap(), action);
}
#[rstest]
#[case::read("thread.read")]
#[case::permission("thread.update")]
#[case::write("title_update")]
#[tokio::test]
async fn title_update_errors_keep_their_opaque_identity(
	mut scope: Scope,
	#[case] fail: &'static str,
) {
	scope.fail = Some(fail);
	let Error::Port(error) = update(&mut scope, Uuid::from_u128(1), 1, "Renamed")
		.await
		.unwrap_err()
	else {
		panic!("expected opaque error");
	};
	assert!(error.is::<std::io::Error>());
	assert!(scope.title_writes.is_empty());
}
#[rstest]
#[case::unkeyed(None)]
#[case::keyed(Some("retry-key"))]
#[case::empty_key(Some(""))]
#[tokio::test]
async fn task_creation_keeps_authorization_reference_checks_and_post_write_disclosure(
	mut scope: Scope,
	#[case] key: Option<&str>,
) {
	let input = input();
	let result = create_task(&mut scope, Uuid::from_u128(1), &input, key)
		.await
		.unwrap();
	assert_eq!(result.id, scope.task.id);
	assert_eq!(
		scope.calls,
		vec![
			"thread.read",
			"task.create",
			"related_tasks",
			"task_insert",
			"task_resource",
			"task.read"
		]
	);
	assert_eq!(scope.task_inputs, vec![(json!(input), "primary".into())]);
	let expected = key.map(|key| {
		format!(
			"subject:{}",
			aidash_domain::registry::rules::digest(&json!([
				"tenant",
				"primary",
				Uuid::from_u128(1),
				key
			]))
		)
	});
	assert_eq!(scope.task_keys, vec![expected]);
	assert_eq!(scope.resources[0].1, "task.read");
}
#[rstest]
#[tokio::test]
async fn idempotency_keys_remain_partitioned_by_tenant_subject_and_thread(scope: Scope) {
	let mut keys = vec![];
	for index in 0..4 {
		let mut candidate = Scope {
			tenant: scope.tenant.clone(),
			subject: scope.subject.clone(),
			thread: scope.thread.clone(),
			task: scope.task.clone(),
			calls: vec![],
			resources: vec![],
			task_keys: vec![],
			task_inputs: vec![],
			messages: vec![],
			title_writes: vec![],
			denied: None,
			fail: None,
		};
		match index {
			1 => candidate.tenant = "other-tenant".into(),
			2 => candidate.subject = "other-subject".into(),
			3 => candidate.thread.id = Uuid::from_u128(3),
			_ => {}
		}
		candidate.task.thread_id = candidate.thread.id;
		candidate.task.created_by = candidate.subject.clone();
		let id = candidate.thread.id;
		create_task(&mut candidate, id, &input(), Some("retry"))
			.await
			.unwrap();
		keys.push(candidate.task_keys.remove(0).unwrap());
	}
	let unique: std::collections::BTreeSet<_> = keys.iter().collect();
	assert_eq!(unique.len(), 4);
	assert!(keys.iter().all(|key| key.starts_with("subject:")));
}
#[rstest]
#[case::thread("thread.read")]
#[case::create("task.create")]
#[case::disclosure("task.read")]
#[tokio::test]
async fn task_creation_and_returned_record_use_current_authority(
	mut scope: Scope,
	#[case] action: &'static str,
) {
	scope.denied = Some(action);
	assert!(matches!(
		create_task(&mut scope, Uuid::from_u128(1), &input(), None).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls.last().unwrap(), action);
	assert_eq!(scope.task_inputs.len(), usize::from(action == "task.read"));
}
#[rstest]
#[case::read("thread.read")]
#[case::create("task.create")]
#[case::references("related_tasks")]
#[case::insert("task_insert")]
#[case::resource("task_resource")]
#[case::disclosure("task.read")]
#[tokio::test]
async fn task_creation_errors_keep_their_opaque_identity(
	mut scope: Scope,
	#[case] fail: &'static str,
) {
	scope.fail = Some(fail);
	let Error::Port(error) = create_task(&mut scope, Uuid::from_u128(1), &input(), Some("retry"))
		.await
		.unwrap_err()
	else {
		panic!("expected opaque error");
	};
	assert!(error.is::<std::io::Error>());
	assert_eq!(scope.calls.last().unwrap(), fail);
}
#[rstest]
#[tokio::test]
async fn message_creation_keeps_the_existing_sender_and_idempotency_namespace(mut scope: Scope) {
	message(
		&mut scope,
		Uuid::from_u128(1),
		"日本語\u{0000}",
		Uuid::from_u128(42),
	)
	.await
	.unwrap();
	assert_eq!(scope.calls, vec!["message.create", "message_insert"]);
	assert_eq!(
		scope.messages,
		vec![(
			"primary".into(),
			"日本語\u{0000}".into(),
			format!(
				"thread-subject:tenant:primary:{}:{}",
				Uuid::from_u128(1),
				Uuid::from_u128(42)
			)
		)]
	);
}
#[rstest]
#[tokio::test]
async fn message_policy_denial_prevents_writing_or_emitting_the_record(mut scope: Scope) {
	scope.denied = Some("message.create");
	assert!(matches!(
		message(&mut scope, Uuid::from_u128(1), "text", Uuid::from_u128(42)).await,
		Err(Error::Forbidden)
	));
	assert!(scope.messages.is_empty());
	assert_eq!(scope.calls, vec!["message.create"]);
}
#[rstest]
#[case::policy("message.create")]
#[case::insert("message_insert")]
#[tokio::test]
async fn message_creation_errors_keep_their_opaque_identity(
	mut scope: Scope,
	#[case] fail: &'static str,
) {
	scope.fail = Some(fail);
	let Error::Port(error) = message(&mut scope, Uuid::from_u128(1), "text", Uuid::from_u128(42))
		.await
		.unwrap_err()
	else {
		panic!("expected opaque error");
	};
	assert!(error.is::<std::io::Error>());
	assert!(scope.messages.is_empty());
}
