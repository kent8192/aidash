use super::*;
use crate::{authorization::Snapshot, ports::generation::foreign::ForeignGenerationGuard};
use aidash_domain::{
	Task,
	federation::execution::Description,
	generation::{
		intent::guards::{Authority, Record},
		remote::Ancestor,
		requests::Request,
	},
	policy::{PolicyBundle, Resource},
	registry::EntityRef,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rstest::{fixture, rstest};
use serde_json::Value;
use std::sync::{Arc, Mutex};

struct World {
	task: Task,
	record: Option<Record>,
	snapshot: Snapshot,
	prepared: Value,
	events: Vec<String>,
	leases: usize,
	denied: Option<&'static str>,
	failure: Option<&'static str>,
	pause: Option<&'static str>,
	current_patch: Option<&'static str>,
	saved_patch: Option<&'static str>,
	lineage: Vec<Ancestor>,
}
struct Repository {
	principal: Principal,
	world: Arc<Mutex<World>>,
	now: DateTime<Utc>,
}
struct Scope {
	world: Arc<Mutex<World>>,
	authority: Authority,
	snapshot: Snapshot,
	pending: Option<Record>,
	now: DateTime<Utc>,
}
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
		let mut w = self.world.lock().unwrap();
		w.leases -= 1;
		w.events.push("drop".into());
	}
}
impl Repository {
	fn scope(&self, record: Option<&Record>) -> Box<dyn HomeGenerationScope> {
		let mut w = self.world.lock().unwrap();
		w.leases += 1;
		Box::new(Scope {
			world: self.world.clone(),
			authority: Authority {
				tenant: "tenant".into(),
				credential_id: record.map_or(Uuid::from_u128(3), |r| r.credential_id),
				subjects: vec![record.map_or_else(|| "root".into(), |r| r.root_subject.clone())],
			},
			snapshot: w.snapshot.clone(),
			pending: None,
			now: self.now,
		})
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
	let bundle:PolicyBundle=serde_json::from_value(json!({"tenant":"tenant","subjects":{"root":{"kind":"user","delegated_by":null},"parent":{"kind":"agent","delegated_by":"root"}}})).unwrap();
	let prepared = json!({"intent_id":Uuid::from_u128(4),"node_id":"aidash://executor","request_id":Uuid::from_u128(5),"agent":{"id":"agent","version":"1.0.0"},"status":"QUEUED","prepared":true,"expires_at":DateTime::<Utc>::from_timestamp(2000,0).unwrap()});
	Repository {
		principal: Principal::Subject {
			tenant: "tenant".into(),
			subject: "root".into(),
		},
		now,
		world: Arc::new(Mutex::new(World {
			task,
			record: None,
			snapshot: Snapshot {
				revision: 9,
				bundle,
			},
			prepared,
			events: vec![],
			leases: 0,
			denied: None,
			failure: None,
			pause: None,
			current_patch: None,
			saved_patch: None,
			lineage: vec![],
		})),
	}
}
fn input() -> Input {
	Input {
		id: Uuid::from_u128(4),
		node_id: "aidash://executor".into(),
		policy_id: "policy".into(),
		policy_revision: 7,
		ttl_seconds: 1000,
		reason: "Generate".into(),
	}
}
fn patch(record: &mut Record, field: &str) {
	match field {
		"cancelled" => record.cancelled = true,
		"tenant" => record.tenant = "other".into(),
		"credential" => record.credential_id = Uuid::from_u128(99),
		"chain" => record.subject_chain = vec!["other".into()],
		"binding" => record.binding["policy_revision"] = json!(99),
		"expiry" => {
			record.binding["expires_at"] = json!(DateTime::<Utc>::from_timestamp(1500, 0).unwrap())
		}
		_ => panic!("unknown record fence"),
	}
}
#[async_trait]
impl HomeGenerationRepository for Repository {
	fn credential_id(&self) -> Option<Uuid> {
		Some(Uuid::from_u128(3))
	}
	fn principal(&self) -> &Principal {
		&self.principal
	}
	fn node_id(&self) -> &str {
		"aidash://home"
	}
	fn now(&self) -> DateTime<Utc> {
		self.now
	}
	async fn load(&self, id: Uuid) -> Result<Record> {
		assert_eq!(id, input().id);
		point(&self.world, "load").await?;
		self.world
			.lock()
			.unwrap()
			.record
			.clone()
			.ok_or(Error::Forbidden)
	}
	async fn begin(&self) -> Result<Box<dyn HomeGenerationScope>> {
		point(&self.world, "begin").await?;
		Ok(self.scope(None))
	}
	async fn begin_saved(
		&self,
		record: &Record,
		exclusive: bool,
	) -> Result<Box<dyn HomeGenerationScope>> {
		point(&self.world, if exclusive { "exclusive" } else { "shared" }).await?;
		assert_eq!(record.root_subject, "root");
		Ok(self.scope(Some(record)))
	}
	async fn prepare(&self, target: &str, id: Uuid) -> Result<Prepared> {
		assert_eq!((target, id), (input().node_id.as_str(), input().id));
		{
			let w = self.world.lock().unwrap();
			assert!(w.record.is_some());
			assert_eq!(w.leases, 0);
		}
		point(&self.world, "rpc").await?;
		Ok(serde_json::from_value(
			self.world.lock().unwrap().prepared.clone(),
		)?)
	}
}
#[async_trait]
impl ForeignGenerationGuard for Scope {
	fn authority(&self) -> Authority {
		self.authority.clone()
	}
	fn now(&self) -> DateTime<Utc> {
		self.now
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		Resource {
			tenant: self.authority.tenant.clone(),
			kind: kind.into(),
			id: id.into(),
			attributes,
		}
	}
	async fn job(&mut self, _: &EntityRef) -> Result<Option<Request>> {
		panic!("Home intent must not inspect a local executor")
	}
	async fn intent(&mut self, _: Uuid) -> Result<Option<Record>> {
		panic!("Home lease requires the fetch-one current row")
	}
	async fn active(&mut self, _: &Description, _: Uuid) -> Result<bool> {
		panic!("Home intent must not create a local admission")
	}
	async fn lineage(&mut self, node: &str) -> Result<Vec<Ancestor>> {
		assert_eq!(node, "aidash://home");
		point(&self.world, "lineage").await?;
		Ok(self.world.lock().unwrap().lineage.clone())
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		Ok(self.resource("workspace", &id.to_string(), json!({})))
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		Ok(self.resource(
			"task",
			&task.id.to_string(),
			json!({"workspace_id":task.workspace_id}),
		))
	}
	async fn require(&mut self, _: &Resource, action: &str) -> Result<()> {
		let denied = {
			let mut w = self.world.lock().unwrap();
			w.events.push(action.into());
			w.denied
		};
		if denied == Some(action) {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
}
#[async_trait]
impl HomeGenerationScope for Scope {
	async fn set_cancelled(&mut self, id: Uuid) -> Result<()> {
		assert_eq!(id, input().id);
		point(&self.world, "cancel").await?;
		let mut record = self.world.lock().unwrap().record.clone().unwrap();
		record.cancelled = true;
		self.pending = Some(record);
		Ok(())
	}
	fn replace_subjects(&mut self, subjects: Vec<String>) {
		self.authority.subjects = subjects;
	}
	fn snapshot(&self) -> &Snapshot {
		&self.snapshot
	}
	fn snapshot_mut(&mut self) -> &mut Snapshot {
		&mut self.snapshot
	}
	async fn inherit_task_origin(&mut self, id: Uuid) -> Result<()> {
		assert_eq!(id, Uuid::from_u128(1));
		point(&self.world, "inherit").await?;
		self.authority.subjects.push("parent".into());
		Ok(())
	}
	async fn task(&mut self, id: Uuid) -> Result<Task> {
		point(&self.world, "task").await?;
		let task = self.world.lock().unwrap().task.clone();
		assert_eq!(id, task.id);
		Ok(task)
	}
	async fn insert(&mut self, intent: &Intent) -> Result<()> {
		point(&self.world, "insert").await?;
		let w = self.world.lock().unwrap();
		self.pending = Some(w.record.clone().unwrap_or_else(|| Record {
			tenant: self.authority.tenant.clone(),
			credential_id: self.authority.credential_id,
			root_subject: "root".into(),
			subject_chain: self.authority.subjects.clone(),
			binding: json!(intent),
			cancelled: false,
		}));
		Ok(())
	}
	async fn saved(&mut self, id: Uuid) -> Result<Record> {
		assert_eq!(id, input().id);
		point(&self.world, "saved").await?;
		let mut record = self.pending.clone().unwrap();
		if let Some(field) = self.world.lock().unwrap().saved_patch {
			patch(&mut record, field);
		}
		Ok(record)
	}
	async fn current(&mut self, id: Uuid) -> Result<Record> {
		assert_eq!(id, input().id);
		point(&self.world, "current").await?;
		let w = self.world.lock().unwrap();
		let mut record = w.record.clone().unwrap();
		if let Some(field) = w.current_patch {
			patch(&mut record, field);
		}
		Ok(record)
	}
	async fn save_snapshot(&mut self) -> Result<()> {
		point(&self.world, "save_snapshot").await
	}
	async fn finish(self: Box<Self>) -> Result<()> {
		point(&self.world, "commit").await?;
		let mut w = self.world.lock().unwrap();
		if let Some(record) = &self.pending {
			w.record = Some(record.clone());
		}
		w.snapshot = self.snapshot.clone();
		Ok(())
	}
	async fn abort(self: Box<Self>, error: Error) -> Error {
		self.world.lock().unwrap().events.push("rollback".into());
		error
	}
}
#[rstest]
#[tokio::test]
async fn request_commits_before_rpc_then_revalidates_exclusively(repository: Repository) {
	let prepared = request(&repository, Uuid::from_u128(1), input())
		.await
		.unwrap();
	let w = repository.world.lock().unwrap();
	assert_eq!(prepared.request_id, Uuid::from_u128(5));
	assert_eq!(w.leases, 0);
	assert_eq!(w.snapshot.revision, 10);
	let executor =
		&w.snapshot.bundle.subjects[&qualified_agent("aidash://executor", "agent", "1.0.0")];
	assert_eq!(executor.delegated_by.as_deref(), Some("parent"));
	assert_eq!(
		executor.attributes,
		json!({"generation_intent":input().id,"remote_node":input().node_id})
	);
	assert_eq!(
		w.record.as_ref().unwrap().subject_chain,
		vec!["root", "parent"]
	);
	assert!(
		w.events.iter().position(|e| e == "commit").unwrap()
			< w.events.iter().position(|e| e == "rpc").unwrap()
	);
	assert!(
		w.events.iter().position(|e| e == "rpc").unwrap()
			< w.events.iter().position(|e| e == "exclusive").unwrap()
	);
}
#[rstest]
#[tokio::test]
async fn identical_retry_preserves_expiry_and_subject_revision(mut repository: Repository) {
	request(&repository, Uuid::from_u128(1), input())
		.await
		.unwrap();
	let original = repository
		.world
		.lock()
		.unwrap()
		.record
		.clone()
		.unwrap()
		.binding;
	repository.now += chrono::Duration::seconds(100);
	request(&repository, Uuid::from_u128(1), input())
		.await
		.unwrap();
	let w = repository.world.lock().unwrap();
	assert_eq!(w.record.as_ref().unwrap().binding, original);
	assert_eq!(w.snapshot.revision, 10);
}
#[rstest]
#[case("workspace.read")]
#[case("generation.disclose")]
#[case("task.read")]
#[case("task.delegate")]
#[case("federation.execute")]
#[case("generation.request")]
#[tokio::test]
async fn permission_denial_rolls_back_before_any_rpc(
	repository: Repository,
	#[case] action: &'static str,
) {
	repository.world.lock().unwrap().denied = Some(action);
	assert!(matches!(
		request(&repository, Uuid::from_u128(1), input()).await,
		Err(Error::Forbidden)
	));
	let w = repository.world.lock().unwrap();
	assert!(w.record.is_none());
	assert_eq!(w.leases, 0);
	assert_eq!(w.events.last().unwrap(), "drop");
	assert!(w.events.contains(&"rollback".into()));
	assert!(!w.events.contains(&"rpc".into()));
}
#[rstest]
#[case("cancelled")]
#[case("tenant")]
#[case("credential")]
#[case("chain")]
#[case("binding")]
#[tokio::test]
async fn conflicting_idempotency_never_commits(
	repository: Repository,
	#[case] field: &'static str,
) {
	repository.world.lock().unwrap().saved_patch = Some(field);
	assert!(
		matches!(request(&repository,Uuid::from_u128(1),input()).await,Err(Error::Conflict(message)) if message=="generation intent already binds different authority or policy")
	);
	let w = repository.world.lock().unwrap();
	assert!(w.record.is_none());
	assert!(!w.events.contains(&"rpc".into()));
}
#[rstest]
#[case("inherit")]
#[case("task")]
#[case("lineage")]
#[case("insert")]
#[case("saved")]
#[case("commit")]
#[tokio::test]
async fn local_failure_preserves_error_and_skips_rpc(
	repository: Repository,
	#[case] operation: &'static str,
) {
	repository.world.lock().unwrap().failure = Some(operation);
	assert!(
		matches!(request(&repository,Uuid::from_u128(1),input()).await,Err(Error::External(message)) if message==format!("{operation} failed"))
	);
	let w = repository.world.lock().unwrap();
	assert!(w.record.is_none());
	assert_eq!(w.leases, 0);
	assert!(!w.events.contains(&"rpc".into()));
}
#[rstest]
#[tokio::test]
async fn rpc_failure_leaves_retryable_committed_intent(repository: Repository) {
	repository.world.lock().unwrap().failure = Some("rpc");
	assert!(
		matches!(request(&repository,Uuid::from_u128(1),input()).await,Err(Error::External(message)) if message=="rpc failed")
	);
	let w = repository.world.lock().unwrap();
	assert!(w.record.is_some());
	assert_eq!(w.snapshot.revision, 9);
	assert_eq!(w.leases, 0);
}
#[rstest]
#[case("intent_id",json!(Uuid::from_u128(99)))]
#[case("node_id",json!("aidash://other"))]
#[tokio::test]
async fn mismatched_prepared_reply_does_not_register_executor(
	repository: Repository,
	#[case] field: &str,
	#[case] value: Value,
) {
	repository.world.lock().unwrap().prepared[field] = value;
	assert!(matches!(
		request(&repository, Uuid::from_u128(1), input()).await,
		Err(Error::Forbidden)
	));
	let w = repository.world.lock().unwrap();
	assert!(w.record.is_some());
	assert_eq!(w.snapshot.revision, 9);
	assert!(!w.events.contains(&"exclusive".into()));
}
#[rstest]
#[tokio::test]
async fn pending_approval_does_not_register_executor(repository: Repository) {
	{
		let mut w = repository.world.lock().unwrap();
		w.prepared["prepared"] = json!(false);
		w.prepared["status"] = json!("PENDING_APPROVAL");
	}
	assert_eq!(
		request(&repository, Uuid::from_u128(1), input())
			.await
			.unwrap()
			.status,
		"PENDING_APPROVAL"
	);
	let w = repository.world.lock().unwrap();
	assert_eq!(w.snapshot.revision, 9);
	assert!(!w.events.contains(&"exclusive".into()));
}
#[rstest]
#[case("cancelled")]
#[case("credential")]
#[case("chain")]
#[case("binding")]
#[tokio::test]
async fn revalidation_fences_changes_before_subject_registration(
	repository: Repository,
	#[case] field: &'static str,
) {
	repository.world.lock().unwrap().current_patch = Some(field);
	assert!(matches!(
		request(&repository, Uuid::from_u128(1), input()).await,
		Err(Error::Forbidden)
	));
	let w = repository.world.lock().unwrap();
	assert!(w.record.is_some());
	assert_eq!(w.snapshot.revision, 9);
	assert!(w.events.contains(&"rollback".into()));
}
#[rstest]
#[tokio::test]
async fn describe_revalidates_read_scope_without_subject_write(repository: Repository) {
	request(&repository, Uuid::from_u128(1), input())
		.await
		.unwrap();
	repository.world.lock().unwrap().events.clear();
	let intent = describe(&repository, "aidash://executor", input().id)
		.await
		.unwrap();
	let w = repository.world.lock().unwrap();
	assert_eq!(intent.id, input().id);
	assert!(w.events.contains(&"shared".into()));
	assert!(!w.events.contains(&"exclusive".into()));
	assert_eq!(w.snapshot.revision, 10);
}
#[rstest]
#[case("id")]
#[case("node")]
#[case("local")]
#[case("revision")]
#[case("ttl_low")]
#[case("ttl_high")]
#[case("reason")]
#[case("reason_bytes")]
#[tokio::test]
async fn invalid_input_is_rejected_before_begin(repository: Repository, #[case] field: &str) {
	let mut value = input();
	match field {
		"id" => value.id = Uuid::nil(),
		"node" => value.node_id = "invalid".into(),
		"local" => value.node_id = "aidash://home".into(),
		"revision" => value.policy_revision = 0,
		"ttl_low" => value.ttl_seconds = 0,
		"ttl_high" => value.ttl_seconds = 3601,
		"reason" => value.reason = " \t".into(),
		"reason_bytes" => value.reason = "界".repeat(1366),
		_ => panic!("unknown input field"),
	};
	assert!(matches!(
		request(&repository, Uuid::from_u128(1), value).await,
		Err(Error::Domain(aidash_domain::Error::Invalid(_)))
	));
	assert!(repository.world.lock().unwrap().events.is_empty());
}
#[rstest]
#[tokio::test]
async fn operator_cannot_request_a_subject_intent(mut repository: Repository) {
	repository.principal = Principal::Operator;
	assert!(matches!(
		request(&repository, Uuid::from_u128(1), input()).await,
		Err(Error::Forbidden)
	));
	assert!(repository.world.lock().unwrap().events.is_empty());
}

#[rstest]
#[tokio::test]
async fn cancellation_commits_with_management_authority_and_queues_delivery_failure(
	repository: Repository,
) {
	request(&repository, Uuid::from_u128(1), input())
		.await
		.unwrap();
	{
		let mut w = repository.world.lock().unwrap();
		w.events.clear();
		w.denied = Some("task.read");
	}
	let maintenance = super::super::maintenance::tests::repository();
	maintenance.0.lock().unwrap().failure = Some("send");
	assert!(
		cancel(&repository, &maintenance, Uuid::from_u128(1), input().id)
			.await
			.unwrap()
	);
	let w = repository.world.lock().unwrap();
	assert!(w.record.as_ref().unwrap().cancelled);
	assert!(w.events.contains(&"task.delegate".into()));
	assert!(!w.events.contains(&"task.read".into()));
	assert_eq!(
		maintenance.0.lock().unwrap().warnings,
		vec![(input().id, "send failed".into(), false)]
	);
}
#[rstest]
#[tokio::test]
async fn denied_cancellation_never_changes_intent_or_delivers(repository: Repository) {
	request(&repository, Uuid::from_u128(1), input())
		.await
		.unwrap();
	repository.world.lock().unwrap().denied = Some("task.delegate");
	let maintenance = super::super::maintenance::tests::repository();
	assert!(matches!(
		cancel(&repository, &maintenance, Uuid::from_u128(1), input().id).await,
		Err(Error::Forbidden)
	));
	assert!(
		!repository
			.world
			.lock()
			.unwrap()
			.record
			.as_ref()
			.unwrap()
			.cancelled
	);
	assert!(maintenance.0.lock().unwrap().events.is_empty());
}
#[rstest]
#[case("task")]
#[case("tenant")]
#[case("credential")]
#[tokio::test]
async fn cancellation_requires_the_original_task_tenant_and_credential(
	repository: Repository,
	#[case] field: &str,
) {
	request(&repository, Uuid::from_u128(1), input())
		.await
		.unwrap();
	let mut task = Uuid::from_u128(1);
	{
		let mut w = repository.world.lock().unwrap();
		w.events.clear();
		match field {
			"task" => task = Uuid::from_u128(99),
			"tenant" => w.record.as_mut().unwrap().tenant = "other".into(),
			"credential" => w.record.as_mut().unwrap().credential_id = Uuid::from_u128(99),
			_ => panic!("unknown cancel binding"),
		};
	}
	let maintenance = super::super::maintenance::tests::repository();
	assert!(matches!(
		cancel(&repository, &maintenance, task, input().id).await,
		Err(Error::Forbidden)
	));
	assert_eq!(repository.world.lock().unwrap().events, vec!["load"]);
	assert!(maintenance.0.lock().unwrap().events.is_empty());
}
#[rstest]
#[case("insert", false)]
#[case("rpc", true)]
#[case("save_snapshot", true)]
#[tokio::test]
async fn cancellation_drops_open_scope_and_preserves_only_committed_intent(
	repository: Repository,
	#[case] operation: &'static str,
	#[case] committed: bool,
) {
	repository.world.lock().unwrap().pause = Some(operation);
	let outcome = tokio::time::timeout(
		std::time::Duration::from_millis(20),
		request(&repository, Uuid::from_u128(1), input()),
	)
	.await;
	assert!(outcome.is_err());
	let w = repository.world.lock().unwrap();
	assert_eq!(w.record.is_some(), committed);
	assert_eq!(w.snapshot.revision, 9);
	assert_eq!(w.leases, 0);
}
