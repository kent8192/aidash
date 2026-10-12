use super::*;
use crate::{
	generation::test_support, ports::generation::foreign::receiver::ForeignGenerationReceiverScope,
};
use aidash_domain::{
	Task, TaskStatus,
	generation::{
		intent::{Intent, guards::Authority},
		policy::Policy,
		requests::Request,
	},
	policy::Resource,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rstest::{fixture, rstest};
use serde_json::Value;
use std::sync::{Arc, Mutex};

struct World {
	intent: Intent,
	policy: Policy,
	job: Request,
	existing: Option<Request>,
	authority: Authority,
	replayed: bool,
	events: Vec<String>,
	denied: Option<&'static str>,
	failure: Option<&'static str>,
	pause: Option<&'static str>,
	committed: usize,
	leases: usize,
	wrong_assignment: bool,
}
struct Repository(Arc<Mutex<World>>);
struct Scope(Arc<Mutex<World>>);
async fn point(world: &Arc<Mutex<World>>, name: &'static str) -> Result<()> {
	let (failure, pause) = {
		let mut w = world.lock().unwrap();
		w.events.push(name.into());
		(w.failure, w.pause)
	};
	if pause == Some(name) {
		std::future::pending::<()>().await;
	}
	if failure == Some(name) {
		return Err(Error::External(format!("{name} failed")));
	}
	Ok(())
}
impl Drop for Scope {
	fn drop(&mut self) {
		self.0.lock().unwrap().leases -= 1;
	}
}
#[fixture]
fn repository() -> Repository {
	let now = DateTime::<Utc>::from_timestamp(1000, 0).unwrap();
	let task = Task {
		id: Uuid::from_u128(1),
		workspace_id: Uuid::from_u128(2),
		title: "Task".into(),
		description: "Work".into(),
		status: TaskStatus::Open,
		requirements: json!({}),
		owner: None,
		created_by: "root".into(),
		dependencies: vec![],
		parent_id: None,
		revision: 7,
		created_at: now,
	};
	let intent = Intent {
		id: Uuid::from_u128(4),
		home_node: "aidash://home".into(),
		source_tenant: "source".into(),
		source_subject: "root".into(),
		task,
		target_node: "aidash://executor".into(),
		policy_id: "policy".into(),
		policy_revision: 7,
		lineage: vec![],
		reason: "Generate".into(),
		ttl_seconds: 1000,
		expires_at: DateTime::from_timestamp(2000, 0).unwrap(),
	};
	let mut job = test_support::request(9);
	job.task_id = intent.task.id;
	job.home_node = intent.home_node.clone();
	job.status = "QUEUED".into();
	job.prepared = false;
	job.foreign_intent = Some(json!(intent));
	job.subject_chain = vec!["mapped".into()];
	job.credential_id = Uuid::from_u128(3);
	let authority = Authority {
		tenant: job.tenant.clone(),
		credential_id: job.credential_id,
		subjects: job.subject_chain.clone(),
	};
	let policy = Policy {
		tenant: job.tenant.clone(),
		id: "policy".into(),
		revision: 7,
		spec: serde_json::from_value(test_support::specification()).unwrap(),
		generated_count: 0,
		allocated_tokens: 0,
		allocated_compaction_calls: 0,
		allocated_embedding_calls: 0,
		allocated_summary_calls: 0,
	};
	Repository(Arc::new(Mutex::new(World {
		intent,
		policy,
		job,
		existing: None,
		authority,
		replayed: false,
		events: vec![],
		denied: None,
		failure: None,
		pause: None,
		committed: 0,
		leases: 0,
		wrong_assignment: false,
	})))
}
#[async_trait]
impl ForeignGenerationReceiver for Repository {
	fn node_id(&self) -> &str {
		"aidash://executor"
	}
	fn now(&self) -> DateTime<Utc> {
		DateTime::from_timestamp(1000, 0).unwrap()
	}
	async fn describe(&self, source: &str, id: Uuid) -> Result<Intent> {
		assert_eq!((source, id), ("aidash://home", Uuid::from_u128(4)));
		point(&self.0, "describe").await?;
		Ok(self.0.lock().unwrap().intent.clone())
	}
	async fn begin(
		&self,
		source: &str,
		tenant: &str,
		subject: &str,
	) -> Result<Box<dyn ForeignGenerationReceiverScope>> {
		assert_eq!(
			(source, tenant, subject),
			("aidash://home", "source", "root")
		);
		point(&self.0, "begin").await?;
		self.0.lock().unwrap().leases += 1;
		Ok(Box::new(Scope(self.0.clone())))
	}
}
#[async_trait]
impl ForeignGenerationReceiverScope for Scope {
	fn authority(&self) -> Authority {
		self.0.lock().unwrap().authority.clone()
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		Resource {
			tenant: self.authority().tenant,
			kind: kind.into(),
			id: id.into(),
			attributes,
		}
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		let mut w = self.0.lock().unwrap();
		w.events.push(action.into());
		match action {
			"federation.execute" => assert_eq!(resource.id, "aidash://executor"),
			"task.read" => assert_eq!(
				resource.id,
				format!("aidash://home/tasks/{}", w.intent.task.id)
			),
			_ => assert_eq!(resource.id, "policy"),
		};
		if w.denied == Some(action) {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	async fn policy(&mut self, id: &str) -> Result<Policy> {
		assert_eq!(id, "policy");
		point(&self.0, "policy").await?;
		Ok(self.0.lock().unwrap().policy.clone())
	}
	async fn existing(&mut self, source: &str, task: Uuid) -> Result<Option<Request>> {
		assert_eq!((source, task), ("aidash://home", Uuid::from_u128(1)));
		point(&self.0, "existing").await?;
		Ok(self.0.lock().unwrap().existing.clone())
	}
	async fn replayed(&mut self, source: &str, id: Uuid) -> Result<bool> {
		assert_eq!((source, id), ("aidash://home", Uuid::from_u128(4)));
		point(&self.0, "replayed").await?;
		Ok(self.0.lock().unwrap().replayed)
	}
	async fn create(&mut self, intent: &Intent, policy: Policy) -> Result<Assignment> {
		point(&self.0, "create").await?;
		let w = self.0.lock().unwrap();
		assert_eq!(json!(intent), json!(w.intent));
		assert_eq!(policy.revision, 7);
		if w.wrong_assignment {
			Ok(Assignment::Existing {
				delegation: aidash_domain::federation::Delegation {
					task_id: intent.task.id,
					node_id: "aidash://executor".into(),
					agent_id: "existing".into(),
					agent_version: "1.0.0".into(),
					delivered: false,
				},
			})
		} else {
			Ok(Assignment::Generated {
				generation: Box::new(w.job.clone()),
			})
		}
	}
	async fn publish(
		&mut self,
		job: &Request,
		spec: &aidash_domain::generation::policy::Spec,
	) -> Result<()> {
		assert_eq!(job.id, Uuid::from_u128(9));
		assert!(spec.enabled);
		point(&self.0, "publish").await
	}
	async fn mark_prepared(&mut self, id: Uuid) -> Result<()> {
		assert_eq!(id, Uuid::from_u128(9));
		point(&self.0, "mark_prepared").await
	}
	async fn finish(self: Box<Self>, result: Result<Prepared>) -> Result<Prepared> {
		if result.is_ok() {
			point(&self.0, "commit").await?;
			self.0.lock().unwrap().committed += 1;
		} else {
			self.0.lock().unwrap().events.push("rollback".into());
		}
		result
	}
}
#[rstest]
#[tokio::test]
async fn prepared_foreign_executor_is_published_once_inside_its_scope(repository: Repository) {
	let prepared = prepare(&repository, "aidash://home", Uuid::from_u128(4))
		.await
		.unwrap();
	assert!(prepared.prepared);
	assert_eq!(prepared.request_id, Uuid::from_u128(9));
	assert_eq!(prepared.status, "QUEUED");
	let w = repository.0.lock().unwrap();
	assert_eq!(w.committed, 1);
	assert_eq!(w.leases, 0);
	assert_eq!(
		w.events,
		vec![
			"describe",
			"begin",
			"federation.execute",
			"task.read",
			"generation.request",
			"generation.read",
			"policy",
			"existing",
			"replayed",
			"create",
			"publish",
			"mark_prepared",
			"commit"
		]
	);
}
#[rstest]
#[case("id",json!(Uuid::from_u128(99)))]
#[case("home_node",json!("aidash://other"))]
#[case("target_node",json!("aidash://other"))]
#[case("expires_at",json!(DateTime::<Utc>::from_timestamp(1000,0).unwrap()))]
#[tokio::test]
async fn invalid_home_description_is_rejected_before_mapped_authority(
	repository: Repository,
	#[case] field: &str,
	#[case] value: Value,
) {
	{
		let mut w = repository.0.lock().unwrap();
		let mut intent = json!(w.intent);
		intent[field] = value;
		w.intent = serde_json::from_value(intent).unwrap();
	}
	assert!(matches!(
		prepare(&repository, "aidash://home", Uuid::from_u128(4)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(repository.0.lock().unwrap().events, vec!["describe"]);
}
#[rstest]
#[case("federation.execute")]
#[case("task.read")]
#[case("generation.request")]
#[case("generation.read")]
#[tokio::test]
async fn current_receiver_denial_prevents_request_and_publication(
	repository: Repository,
	#[case] action: &'static str,
) {
	repository.0.lock().unwrap().denied = Some(action);
	assert!(matches!(
		prepare(&repository, "aidash://home", Uuid::from_u128(4)).await,
		Err(Error::Forbidden)
	));
	let w = repository.0.lock().unwrap();
	assert_eq!(w.committed, 0);
	assert_eq!(w.leases, 0);
	assert_eq!(w.events.last().unwrap(), "rollback");
	assert!(!w.events.contains(&"create".into()));
}
#[rstest]
#[case(false, 7)]
#[case(true, 8)]
#[tokio::test]
async fn policy_must_remain_enabled_at_the_exact_revision(
	repository: Repository,
	#[case] enabled: bool,
	#[case] revision: i64,
) {
	{
		let mut w = repository.0.lock().unwrap();
		w.policy.spec.enabled = enabled;
		w.policy.revision = revision;
	}
	assert!(
		matches!(prepare(&repository,"aidash://home",Uuid::from_u128(4)).await,Err(Error::Conflict(message)) if message=="generation policy revision changed")
	);
	let w = repository.0.lock().unwrap();
	assert!(!w.events.contains(&"existing".into()));
	assert_eq!(w.committed, 0);
}
#[rstest]
#[case("foreign_intent")]
#[case("credential")]
#[case("chain")]
#[case("tenant")]
#[tokio::test]
async fn existing_request_requires_the_exact_mapped_binding(
	repository: Repository,
	#[case] field: &str,
) {
	{
		let mut w = repository.0.lock().unwrap();
		let mut job = w.job.clone();
		match field {
			"foreign_intent" => job.foreign_intent = None,
			"credential" => job.credential_id = Uuid::from_u128(99),
			"chain" => job.subject_chain.clear(),
			"tenant" => job.tenant = "other".into(),
			_ => panic!("unknown binding field"),
		};
		w.existing = Some(job);
	}
	assert!(
		matches!(prepare(&repository,"aidash://home",Uuid::from_u128(4)).await,Err(Error::Conflict(message)) if message=="foreign generation already has a different binding")
	);
	let w = repository.0.lock().unwrap();
	assert_eq!(w.committed, 0);
	assert!(!w.events.contains(&"create".into()));
}
#[rstest]
#[case("PENDING_APPROVAL", false)]
#[case("QUEUED", true)]
#[case("ACTIVE", true)]
#[tokio::test]
async fn idempotent_pending_and_prepared_replies_skip_publication(
	repository: Repository,
	#[case] status: &str,
	#[case] prepared: bool,
) {
	{
		let mut w = repository.0.lock().unwrap();
		let mut job = w.job.clone();
		job.status = status.into();
		job.prepared = prepared;
		w.existing = Some(job);
	}
	let reply = prepare(&repository, "aidash://home", Uuid::from_u128(4))
		.await
		.unwrap();
	assert_eq!(reply.status, status);
	assert_eq!(reply.prepared, prepared);
	let w = repository.0.lock().unwrap();
	assert_eq!(w.committed, 1);
	assert!(!w.events.contains(&"replayed".into()));
	assert!(!w.events.contains(&"create".into()));
	assert!(!w.events.contains(&"publish".into()));
}
#[rstest]
#[tokio::test]
async fn terminal_intent_cannot_create_a_second_executor(repository: Repository) {
	repository.0.lock().unwrap().replayed = true;
	assert!(
		matches!(prepare(&repository,"aidash://home",Uuid::from_u128(4)).await,Err(Error::Conflict(message)) if message=="foreign generation intent already finished")
	);
	assert!(
		!repository
			.0
			.lock()
			.unwrap()
			.events
			.contains(&"create".into())
	);
}
#[rstest]
#[tokio::test]
async fn preparation_requires_a_generated_assignment(repository: Repository) {
	repository.0.lock().unwrap().wrong_assignment = true;
	assert!(matches!(
		prepare(&repository, "aidash://home", Uuid::from_u128(4)).await,
		Err(Error::Forbidden)
	));
	let w = repository.0.lock().unwrap();
	assert_eq!(w.committed, 0);
	assert!(!w.events.contains(&"publish".into()));
}
#[rstest]
#[case("policy")]
#[case("existing")]
#[case("replayed")]
#[case("create")]
#[case("publish")]
#[case("mark_prepared")]
#[case("commit")]
#[tokio::test]
async fn preparation_failure_preserves_error_and_does_not_commit(
	repository: Repository,
	#[case] operation: &'static str,
) {
	repository.0.lock().unwrap().failure = Some(operation);
	assert!(
		matches!(prepare(&repository,"aidash://home",Uuid::from_u128(4)).await,Err(Error::External(message)) if message==format!("{operation} failed"))
	);
	let w = repository.0.lock().unwrap();
	assert_eq!(w.committed, 0);
	assert_eq!(w.leases, 0);
}
#[rstest]
#[case("create")]
#[case("publish")]
#[case("mark_prepared")]
#[tokio::test]
async fn cancelled_preparation_releases_its_uncommitted_scope(
	repository: Repository,
	#[case] operation: &'static str,
) {
	repository.0.lock().unwrap().pause = Some(operation);
	assert!(
		tokio::time::timeout(
			std::time::Duration::from_millis(20),
			prepare(&repository, "aidash://home", Uuid::from_u128(4))
		)
		.await
		.is_err()
	);
	let w = repository.0.lock().unwrap();
	assert_eq!(w.committed, 0);
	assert_eq!(w.leases, 0);
}
