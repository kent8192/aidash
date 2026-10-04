use super::*;
use crate::ports::capabilities::cleanup::ErasureScope;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
struct State {
	area: Area,
	record: Record,
	objects: Vec<Uuid>,
	erased: Vec<Uuid>,
	events: Vec<Value>,
	commits: usize,
	rollbacks: usize,
	open: usize,
	fail_update: bool,
}
struct Repository(Arc<Mutex<State>>);
struct Scope {
	state: Arc<Mutex<State>>,
	area: Option<Area>,
	record: Option<Record>,
	erased: Vec<Uuid>,
	events: Vec<Value>,
	committed: bool,
}
impl Drop for Scope {
	fn drop(&mut self) {
		let mut state = self.state.lock().unwrap();
		state.open -= 1;
		if !self.committed {
			state.rollbacks += 1;
		}
	}
}
#[async_trait]
impl CleanupRepository for Repository {
	async fn jobs(&self, _: Uuid) -> Result<Vec<Record>> {
		Ok(vec![])
	}
	async fn begin(&self) -> Result<Box<dyn ErasureScope + '_>> {
		self.0.lock().unwrap().open += 1;
		Ok(Box::new(Scope {
			state: self.0.clone(),
			area: None,
			record: None,
			erased: vec![],
			events: vec![],
			committed: false,
		}))
	}
	async fn failure(&self, _: Uuid, _: Option<Uuid>) -> Result<()> {
		Ok(())
	}
}
#[async_trait]
impl ErasureScope for Scope {
	async fn locked(&mut self, _: &Record) -> Result<(Area, Record)> {
		let state = self.state.lock().unwrap();
		Ok((state.area.clone(), state.record.clone()))
	}
	async fn objects(&mut self, _: &Area) -> Result<Vec<Uuid>> {
		Ok(self.state.lock().unwrap().objects.clone())
	}
	async fn erase(&mut self, _: &str, id: Uuid) -> Result<()> {
		self.erased.push(id);
		Ok(())
	}
	async fn update_area(&mut self, area: &Area) -> Result<()> {
		self.area = Some(area.clone());
		Ok(())
	}
	async fn update_record(&mut self, record: &mut Record) -> Result<()> {
		if self.state.lock().unwrap().fail_update {
			return Err(Error::External("record write failed".into()));
		}
		self.record = Some(record.clone());
		Ok(())
	}
	async fn event(&mut self, _: Uuid, kind: &str, data: Value) -> Result<()> {
		assert_eq!(kind, "capability.cleanup_changed");
		self.events.push(data);
		Ok(())
	}
	async fn finish(mut self: Box<Self>, result: Result<bool>) -> Result<()> {
		match result {
			Ok(true) => {
				let mut state = self.state.lock().unwrap();
				state.area = self.area.take().unwrap();
				state.record = self.record.take().unwrap();
				state.erased.append(&mut self.erased);
				state.events.append(&mut self.events);
				state.commits += 1;
				self.committed = true;
				Ok(())
			}
			Ok(false) => Ok(()),
			Err(error) => Err(error),
		}
	}
}
fn fixture(state: &str) -> Repository {
	let area = Area {
		id: Uuid::new_v4(),
		tenant: "tenant".into(),
		home_node: "home".into(),
		workspace_id: Uuid::new_v4(),
		thread_id: Uuid::new_v4(),
		agent_id: "agent".into(),
		owner: "alice".into(),
		generation: 7,
		revision: 4,
		epoch: 2,
		state: "cleaning".into(),
		manifest: json!([]),
		constraints: json!([]),
		next_sequence: 1,
	};
	let snapshot = Uuid::new_v4();
	let reference = Uuid::new_v4();
	let replaced = Uuid::new_v4();
	let record = Record {
		id: Uuid::new_v4(),
		tenant: area.tenant.clone(),
		owner: area.owner.clone(),
		area_id: Some(area.id),
		kind: "cleanup".into(),
		state: state.into(),
		revision: 0,
		data: json!({"generation":7,"choice":"recoverable","snapshot":[{"file_id":snapshot,"path":"kept.txt","digest":"digest","size":3,"media_type":"text/plain","scope":"working","provenance":{}},{"file_id":reference,"path":"reference.txt","digest":"digest","size":3,"media_type":"text/plain","scope":"references","provenance":{}}]}),
		expires_at: Some(Utc::now() - Duration::seconds(1)),
	};
	Repository(Arc::new(Mutex::new(State {
		area,
		record,
		objects: vec![snapshot, replaced],
		erased: vec![],
		events: vec![],
		commits: 0,
		rollbacks: 0,
		open: 0,
		fail_update: false,
	})))
}
#[tokio::test]
async fn deleting_preserves_the_recovery_snapshot_and_commits_its_event() {
	let repository = fixture("deleting");
	let snapshot = repository.0.lock().unwrap().record.clone();
	erase_job(&repository, snapshot).await.unwrap();
	let state = repository.0.lock().unwrap();
	assert_eq!(state.record.state, "recoverable");
	assert_eq!(state.area.state, "recoverable");
	assert_eq!(state.erased, vec![state.objects[1]]);
	assert_eq!(state.events.len(), 1);
	assert_eq!(state.commits, 1);
	assert_eq!(state.rollbacks, 0);
	assert_eq!(state.open, 0);
}
#[tokio::test]
async fn record_failure_rolls_back_deletion_and_area_transition() {
	let repository = fixture("deleting");
	let snapshot = {
		let mut state = repository.0.lock().unwrap();
		state.fail_update = true;
		state.record.clone()
	};
	let error = erase_job(&repository, snapshot).await.unwrap_err();
	assert_eq!(error.to_string(), "record write failed");
	let state = repository.0.lock().unwrap();
	assert_eq!(state.record.state, "deleting");
	assert_eq!(state.area.state, "cleaning");
	assert_eq!(state.erased, vec![]);
	assert_eq!(state.events, vec![]);
	assert_eq!(state.commits, 0);
	assert_eq!(state.rollbacks, 1);
	assert_eq!(state.open, 0);
}
#[tokio::test]
async fn kept_snapshot_cannot_be_erased_by_a_previously_selected_job() {
	let repository = fixture("kept");
	let snapshot = repository.0.lock().unwrap().record.clone();
	erase_job(&repository, snapshot).await.unwrap();
	let state = repository.0.lock().unwrap();
	assert_eq!(state.record.state, "kept");
	assert_eq!(state.erased, vec![]);
	assert_eq!(state.commits, 0);
	assert_eq!(state.rollbacks, 1);
	assert_eq!(state.open, 0);
}
#[tokio::test]
async fn recovery_expiry_erases_only_snapshot_owned_bytes_and_advances_the_fence() {
	let repository = fixture("recoverable");
	let snapshot = {
		let mut state = repository.0.lock().unwrap();
		state.area.state = "recoverable".into();
		state.record.clone()
	};
	erase_job(&repository, snapshot).await.unwrap();
	let state = repository.0.lock().unwrap();
	assert_eq!(state.record.state, "expired");
	assert_eq!(state.record.data["snapshot"], json!([]));
	assert_eq!(state.record.expires_at, None);
	assert_eq!(state.erased, vec![state.objects[0]]);
	assert_eq!(state.area.state, "deleted");
	assert_eq!(
		(state.area.generation, state.area.epoch, state.area.revision),
		(8, 3, 5)
	);
	assert_eq!(state.commits, 1);
	assert_eq!(state.open, 0);
}
#[tokio::test]
async fn an_expired_job_cannot_delete_a_new_generation() {
	let repository = fixture("recoverable");
	let snapshot = {
		let mut state = repository.0.lock().unwrap();
		state.area.state = "recoverable".into();
		state.area.generation += 1;
		state.record.clone()
	};
	let error = erase_job(&repository, snapshot).await.unwrap_err();
	assert_eq!(error.to_string(), "CLEANUP_GENERATION_CHANGED");
	let state = repository.0.lock().unwrap();
	assert_eq!(state.record.state, "recoverable");
	assert_eq!(state.erased, vec![]);
	assert_eq!(state.commits, 0);
	assert_eq!(state.rollbacks, 1);
	assert_eq!(state.open, 0);
}
