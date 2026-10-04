use super::*;
use crate::ports::capabilities::python::Limits;
use aidash_domain::capabilities::records::Record;
use async_trait::async_trait;
use rstest::rstest;
struct Scope {
	record: Record,
	confirmed: bool,
	requests: Vec<String>,
	updates: Vec<Record>,
}
#[async_trait]
impl PythonScope for Scope {
	fn subjects(&self) -> &[String] {
		&[]
	}
	fn credential(&self) -> Uuid {
		Uuid::nil()
	}
	fn policy_revision(&self) -> i64 {
		0
	}
	fn limits(&self) -> Result<Limits> {
		panic!("unexpected limits query")
	}
	async fn load(&mut self, _: Uuid) -> Result<Record> {
		Ok(self.record.clone())
	}
	async fn create(&mut self, _: Uuid, _: Value) -> Result<Record> {
		panic!("unexpected creation")
	}
	async fn update(&mut self, record: &mut Record) -> Result<()> {
		self.updates.push(record.clone());
		Ok(())
	}
	async fn cached(&mut self, _: Uuid, _: &str) -> Result<Option<Value>> {
		panic!("unexpected replay lookup")
	}
	async fn cache(&mut self, _: Uuid, _: &str, _: &Value) -> Result<()> {
		panic!("unexpected replay write")
	}
	async fn previous_result(&mut self, _: Uuid) -> Result<Value> {
		panic!("unexpected operation lookup")
	}
	async fn current_run(&mut self, _: &Area) -> Result<Option<Uuid>> {
		panic!("unexpected queue query")
	}
	async fn require_write(&mut self, _: &Area) -> Result<()> {
		panic!("unexpected admission")
	}
	async fn health(&mut self) -> Result<Value> {
		panic!("unexpected health probe")
	}
	async fn request(&mut self, method: &str, path: &str, _: Option<Value>) -> Result<Value> {
		assert_eq!(method, "POST");
		self.requests.push(path.into());
		Ok(json!({"termination_confirmed":self.confirmed}))
	}
	async fn environment(&mut self, _: &Area) -> Result<Value> {
		panic!("unexpected package query")
	}
	async fn prepare(&mut self, _: &mut Area, _: Shell, _: Value) -> Result<Value> {
		panic!("unexpected code replay")
	}
	async fn event(&mut self, _: Uuid, _: &str, _: Value) -> Result<()> {
		panic!("unexpected event")
	}
}
fn fixture(state: &str, confirmed: bool) -> (Scope, Area) {
	let id = Uuid::new_v4();
	let area = Area {
		id,
		tenant: "tenant".into(),
		home_node: "home".into(),
		workspace_id: Uuid::new_v4(),
		thread_id: Uuid::new_v4(),
		agent_id: "agent".into(),
		owner: "alice".into(),
		generation: 7,
		revision: 4,
		epoch: 2,
		state: "active".into(),
		manifest: json!([]),
		constraints: json!([]),
		next_sequence: 1,
	};
	let record = Record {
		id,
		tenant: "tenant".into(),
		owner: "alice".into(),
		area_id: Some(id),
		kind: "python_session".into(),
		state: state.into(),
		revision: 0,
		data: json!({"session_id":"session"}),
		expires_at: None,
	};
	(
		Scope {
			record,
			confirmed,
			requests: vec![],
			updates: vec![],
		},
		area,
	)
}
#[rstest]
#[case("initial")]
#[case("frozen")]
#[case("running")]
#[tokio::test]
async fn reset_waits_for_positive_writer_termination(#[case] state: &str) {
	let (mut scope, area) = fixture(state, true);
	release(&mut scope, &area, "authority_changed")
		.await
		.unwrap();
	assert_eq!(scope.requests, vec!["/v1/sessions/session/stop"]);
	assert_eq!(scope.updates.len(), 1);
	assert_eq!(scope.updates[0].state, "reset");
	assert_eq!(scope.updates[0].data["reset_reason"], "authority_changed");
}
#[tokio::test]
async fn unconfirmed_stop_cannot_mark_a_heap_reset() {
	let (mut scope, area) = fixture("frozen", false);
	let error = release(&mut scope, &area, "authority_changed")
		.await
		.unwrap_err();
	assert_eq!(error.to_string(), "PYTHON_TERMINATION_UNCONFIRMED");
	assert_eq!(scope.requests, vec!["/v1/sessions/session/stop"]);
	assert_eq!(scope.updates.len(), 0);
}
#[tokio::test]
async fn an_already_reset_heap_never_replays_or_stops_old_code() {
	let (mut scope, area) = fixture("reset", true);
	release(&mut scope, &area, "cleanup").await.unwrap();
	assert_eq!(scope.requests, Vec::<String>::new());
	assert_eq!(scope.updates.len(), 1);
	assert_eq!(scope.updates[0].data["reset_reason"], "cleanup");
}
#[tokio::test]
async fn completion_cannot_freeze_another_operation() {
	let (mut scope, area) = fixture("running", true);
	scope.record.data["operation_id"] = json!(Uuid::new_v4());
	let error = completed(
		&mut scope,
		&area,
		Uuid::new_v4(),
		&json!({"writer_frozen":true}),
	)
	.await
	.unwrap_err();
	assert_eq!(error.to_string(), "PYTHON_OPERATION_CHANGED");
	assert_eq!(scope.updates.len(), 0);
	assert_eq!(scope.requests, Vec::<String>::new());
}
