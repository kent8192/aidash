use super::*;
use crate::ports::capabilities::reclamation::ReclamationScope;
use aidash_domain::capabilities::{operations::FileScope, records::Record};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::sync::{Arc, Mutex};

struct State {
	record: Record,
	kind: String,
	calls: Vec<String>,
	scopes: usize,
	quota_failure: bool,
	observed: Value,
}
#[derive(Clone)]
struct Repository(Arc<Mutex<State>>);
struct Scope {
	state: Arc<Mutex<State>>,
	pending: Option<Record>,
}
impl Drop for Scope {
	fn drop(&mut self) {
		self.state.lock().unwrap().scopes -= 1;
	}
}
fn file() -> FileEntry {
	FileEntry {
		file_id: Uuid::from_u128(2),
		path: "original.txt".into(),
		digest: "a".repeat(64),
		size: 1,
		media_type: "text/plain".into(),
		scope: FileScope::References,
		provenance: json!({}),
	}
}
#[fixture]
fn repository() -> Repository {
	Repository(Arc::new(Mutex::new(State {
		record: Record {
			id: Uuid::from_u128(1),
			tenant: "tenant".into(),
			owner: "revoked-user".into(),
			area_id: None,
			kind: "transfer_out".into(),
			state: "pending".into(),
			revision: 1,
			data: json!({"description":{"files":[file()]} }),
			expires_at: Some(Utc::now() - chrono::Duration::seconds(1)),
		},
		kind: "transfer_snapshot".into(),
		calls: vec![],
		scopes: 0,
		quota_failure: false,
		observed: json!({"status":"cancelling","termination_confirmed":false}),
	})))
}
#[async_trait]
impl ReclamationRepository for Repository {
	async fn begin(&self) -> Result<Box<dyn ReclamationScope + '_>> {
		self.0.lock().unwrap().scopes += 1;
		Ok(Box::new(Scope {
			state: self.0.clone(),
			pending: None,
		}))
	}
	async fn retained(&self, _: Uuid) -> Result<Vec<Uuid>> {
		Ok(vec![])
	}
	async fn working(&self, _: Uuid) -> Result<Vec<Uuid>> {
		Ok(vec![])
	}
	async fn python(&self, _: &mut Uuid) -> Result<()> {
		Ok(())
	}
	async fn orphans(&self) -> Result<()> {
		Ok(())
	}
}
#[async_trait]
impl ReclamationScope for Scope {
	async fn load(&mut self, id: Uuid) -> Result<Record> {
		let mut state = self.state.lock().unwrap();
		assert_eq!(id, state.record.id);
		state.calls.push("lock-record".into());
		Ok(state.record.clone())
	}
	async fn release_quota(&mut self, tenant: &str, reserved: i64) -> Result<()> {
		assert_eq!(tenant, "tenant");
		assert_eq!(reserved, 7);
		let mut state = self.state.lock().unwrap();
		state.calls.push("quota".into());
		if state.quota_failure {
			Err(Error::External("quota failed".into()))
		} else {
			Ok(())
		}
	}
	async fn object_kind(&mut self, id: Uuid, tenant: &str) -> Result<Option<String>> {
		assert_eq!(id, file().file_id);
		assert_eq!(tenant, "tenant");
		let mut state = self.state.lock().unwrap();
		state.calls.push("object-kind".into());
		Ok(Some(state.kind.clone()))
	}
	async fn working_tenant(&mut self, _: Uuid) -> Result<Option<String>> {
		Err(Error::Forbidden)
	}
	async fn erase(&mut self, tenant: &str, id: Uuid) -> Result<()> {
		assert_eq!(tenant, "tenant");
		assert_eq!(id, file().file_id);
		self.state.lock().unwrap().calls.push("erase".into());
		Ok(())
	}
	async fn update(&mut self, record: &mut Record) -> Result<()> {
		self.state.lock().unwrap().calls.push("update".into());
		record.revision += 1;
		self.pending = Some(record.clone());
		Ok(())
	}
	async fn request(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
		assert_eq!(method, "POST");
		let mut state = self.state.lock().unwrap();
		state.calls.push(path.into());
		if path.ends_with("/ack") {
			assert_eq!(
				body.unwrap()["digest"],
				aidash_domain::registry::rules::digest(&json!([
					"extract/1",
					state.record.id,
					file()
				]))
			);
			Ok(json!({"acknowledged":true}))
		} else {
			assert!(path.ends_with("/cancel"));
			assert!(body.is_none());
			Ok(state.observed.clone())
		}
	}
	async fn finish(self: Box<Self>, result: Result<bool>) -> Result<()> {
		let mut state = self.state.lock().unwrap();
		match result {
			Ok(true) => {
				state.calls.push("commit".into());
				if let Some(record) = &self.pending {
					state.record = record.clone();
				}
				Ok(())
			}
			Ok(false) => {
				state.calls.push("no-work".into());
				Ok(())
			}
			Err(error) => {
				state.calls.push("rollback".into());
				Err(error)
			}
		}
	}
}

#[rstest]
#[case::unattempted(false, "expired")]
#[case::attempted(true, "expired_unconfirmed")]
#[tokio::test]
async fn expired_transfer_release_commits_ownership_and_preserves_prior_effect_uncertainty(
	repository: Repository,
	#[case] attempted: bool,
	#[case] final_state: &str,
) {
	repository.0.lock().unwrap().record.data["commit_attempted"] = json!(attempted);
	reclaim(&repository, Uuid::from_u128(1)).await.unwrap();
	let state = repository.0.lock().unwrap();
	assert_eq!(state.record.state, final_state);
	assert_eq!(state.record.data["objects_released"], true);
	assert_eq!(
		state.calls,
		vec!["lock-record", "object-kind", "erase", "update", "commit"]
	);
	assert_eq!(state.scopes, 0);
}

#[rstest]
#[tokio::test]
async fn released_records_keep_the_original_no_work_rollback_boundary(repository: Repository) {
	repository.0.lock().unwrap().record.data["objects_released"] = json!(true);
	reclaim(&repository, Uuid::from_u128(1)).await.unwrap();
	let state = repository.0.lock().unwrap();
	assert_eq!(state.calls, vec!["lock-record", "no-work"]);
	assert_eq!(state.record.revision, 1);
	assert_eq!(state.scopes, 0);
}

#[rstest]
#[tokio::test]
async fn an_unexpired_transfer_cannot_release_its_objects(repository: Repository) {
	repository.0.lock().unwrap().record.expires_at = None;
	reclaim(&repository, Uuid::from_u128(1)).await.unwrap();
	let state = repository.0.lock().unwrap();
	assert_eq!(state.calls, vec!["lock-record", "no-work"]);
	assert_eq!(state.scopes, 0);
}

#[rstest]
#[tokio::test]
async fn a_foreign_object_namespace_rolls_back_the_release_marker(repository: Repository) {
	repository.0.lock().unwrap().kind = "working".into();
	assert!(matches!(
		reclaim(&repository, Uuid::from_u128(1)).await,
		Err(Error::Forbidden)
	));
	let state = repository.0.lock().unwrap();
	assert_eq!(state.calls, vec!["lock-record", "object-kind", "rollback"]);
	assert!(state.record.data["objects_released"].is_null());
	assert_eq!(state.scopes, 0);
}

fn inbound(repository: &Repository) {
	let mut state = repository.0.lock().unwrap();
	state.record.kind = "transfer_in".into();
	state.record.data = json!({"chunks":{"one":file()},"reserved":7});
	state.kind = "transfer_staging".into();
}
#[rstest]
#[tokio::test]
async fn inbound_quota_release_precedes_object_deletion_and_commits_with_the_record(
	repository: Repository,
) {
	inbound(&repository);
	reclaim(&repository, Uuid::from_u128(1)).await.unwrap();
	let state = repository.0.lock().unwrap();
	assert_eq!(
		state.calls,
		vec![
			"lock-record",
			"quota",
			"object-kind",
			"erase",
			"update",
			"commit"
		]
	);
	assert_eq!(state.record.state, "expired");
	assert_eq!(state.record.data["reserved"], 0);
	assert_eq!(state.record.data["chunks"], json!({}));
	assert_eq!(state.scopes, 0);
}

#[rstest]
#[tokio::test]
async fn quota_failure_preserves_the_reservation_and_does_not_delete_bytes(repository: Repository) {
	inbound(&repository);
	repository.0.lock().unwrap().quota_failure = true;
	assert!(
		matches!(reclaim(&repository,Uuid::from_u128(1)).await,Err(Error::External(message)) if message=="quota failed")
	);
	let state = repository.0.lock().unwrap();
	assert_eq!(state.calls, vec!["lock-record", "quota", "rollback"]);
	assert_eq!(state.record.data["reserved"], 7);
	assert_eq!(state.scopes, 0);
}

fn reference(repository: &Repository) {
	let mut state = repository.0.lock().unwrap();
	state.record.kind = "reference".into();
	state.record.state = "revoked".into();
	state.record.data = json!({"operation_id":Uuid::from_u128(3),"instance":"runner","original":file(),"extraction":null,"chunks":[]});
	state.kind = "reference_original".into();
}
#[rstest]
#[tokio::test]
async fn an_unconfirmed_parser_stop_cannot_delete_the_reference_original(repository: Repository) {
	reference(&repository);
	assert!(
		matches!(reclaim(&repository,Uuid::from_u128(1)).await,Err(Error::Conflict(message)) if message=="EXTRACTOR_STOP_PENDING")
	);
	let state = repository.0.lock().unwrap();
	assert_eq!(state.calls.len(), 3);
	assert_eq!(state.calls[2], "rollback");
	assert_eq!(state.record.data["original"], json!(file()));
	assert_eq!(state.scopes, 0);
}

#[rstest]
#[tokio::test]
async fn committed_but_undispatched_absence_needs_no_runner_acknowledgement(
	repository: Repository,
) {
	reference(&repository);
	{
		let mut state = repository.0.lock().unwrap();
		state.record.data["dispatch_pending"] = json!(true);
		state.observed = json!({"status":"absent","termination_confirmed":false});
	}
	reclaim(&repository, Uuid::from_u128(1)).await.unwrap();
	let state = repository.0.lock().unwrap();
	assert_eq!(
		state
			.calls
			.iter()
			.filter(|path| path.ends_with("/ack"))
			.count(),
		0
	);
	assert_eq!(state.record.data["runner_released"], true);
	assert_eq!(state.record.data["dispatch_pending"], false);
	assert!(state.record.data["original"].is_null());
	assert_eq!(state.scopes, 0);
}

#[rstest]
#[tokio::test]
async fn positive_termination_is_acknowledged_before_reference_bytes_are_reclaimed(
	repository: Repository,
) {
	reference(&repository);
	repository.0.lock().unwrap().observed =
		json!({"status":"cancelled","termination_confirmed":true});
	reclaim(&repository, Uuid::from_u128(1)).await.unwrap();
	let state = repository.0.lock().unwrap();
	let ack = state
		.calls
		.iter()
		.position(|path| path.ends_with("/ack"))
		.unwrap();
	let erase = state.calls.iter().position(|call| call == "erase").unwrap();
	assert!(ack < erase);
	assert_eq!(state.record.data["runner_released"], true);
	assert_eq!(state.record.data["objects_released"], true);
	assert_eq!(state.record.state, "revoked");
	assert_eq!(state.scopes, 0);
}
