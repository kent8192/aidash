use super::*;
use crate::ports::capabilities::reconciliation::{
	OperationReconciliationRepository, OperationReconciliationScope,
};
use aidash_domain::capabilities::operations::{processing::Receipt, reconciliation::Snapshot};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::{collections::VecDeque, sync::Mutex};
type Row = (Uuid, String, Option<String>);
struct Trace {
	calls: Vec<String>,
	after: Vec<Uuid>,
	rows: VecDeque<Vec<Row>>,
	health: Value,
	ack_failure: bool,
	health_failure: bool,
	mark_failure: bool,
	active_failure: bool,
	block_failure: bool,
	active: Vec<Uuid>,
	quota: bool,
	marked: Vec<Uuid>,
	blocked: Vec<Value>,
}
struct Repository(Mutex<Trace>);
#[fixture]
fn repository() -> Repository {
	Repository(Mutex::new(Trace {
		calls: vec![],
		after: vec![],
		rows: VecDeque::from([vec![(
			Uuid::from_u128(1),
			"digest-one".into(),
			Some("runner".into()),
		)]]),
		health: json!({"instance":"runner"}),
		ack_failure: false,
		health_failure: false,
		mark_failure: false,
		active_failure: false,
		block_failure: false,
		active: vec![],
		quota: false,
		marked: vec![],
		blocked: vec![],
	}))
}
#[async_trait]
impl OperationReconciliationRepository for Repository {
	async fn snapshot(&self, id: Uuid) -> Result<Snapshot> {
		let mut trace = self.0.lock().unwrap();
		trace.calls.push(format!("snapshot:{id}"));
		if trace.quota {
			Err(Error::Domain(aidash_domain::Error::Conflict(
				"runner file quota".into(),
			)))
		} else {
			Err(Error::External("snapshot fixture unavailable".into()))
		}
	}
	async fn begin(&self, _: &Snapshot) -> Result<Box<dyn OperationReconciliationScope + '_>> {
		Err(Error::Forbidden)
	}
	async fn withdraw(&self, _: &Snapshot) -> Result<()> {
		self.0.lock().unwrap().calls.push("withdraw".into());
		Ok(())
	}
}
#[async_trait]
impl OperationProcessingRepository for Repository {
	async fn active_operations(&self) -> Result<Vec<Uuid>> {
		let mut trace = self.0.lock().unwrap();
		trace.calls.push("active".into());
		if trace.active_failure {
			Err(Error::External("scan failed".into()))
		} else {
			Ok(trace.active.clone())
		}
	}
	async fn receipts(&self, after: Uuid) -> Result<Vec<Receipt>> {
		let mut trace = self.0.lock().unwrap();
		trace.calls.push("receipts".into());
		trace.after.push(after);
		Ok(trace
			.rows
			.pop_front()
			.unwrap_or_default()
			.into_iter()
			.map(|(id, digest, runner_instance)| Receipt {
				id,
				digest,
				runner_instance,
			})
			.collect())
	}
	async fn mark_acknowledged(&self, id: Uuid) -> Result<()> {
		let mut trace = self.0.lock().unwrap();
		trace.calls.push("mark".into());
		if trace.mark_failure {
			Err(Error::External("mark failed".into()))
		} else {
			trace.marked.push(id);
			Ok(())
		}
	}
	async fn storage_blocked(&self, id: Uuid, detail: Value) -> Result<()> {
		let mut trace = self.0.lock().unwrap();
		trace.calls.push(format!("blocked:{id}"));
		if trace.block_failure {
			Err(Error::External("block failed".into()))
		} else {
			trace.blocked.push(detail);
			Ok(())
		}
	}
	async fn runner_request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
		let mut trace = self.0.lock().unwrap();
		trace.calls.push(format!("{method}:{path}"));
		if method == "GET" {
			assert!(body.is_none());
			if trace.health_failure {
				Err(Error::External("health failed".into()))
			} else {
				Ok(trace.health.clone())
			}
		} else {
			assert_eq!(body.unwrap()["digest"], json!("digest-one"));
			if trace.ack_failure {
				Err(Error::External("ack failed".into()))
			} else {
				Ok(json!({"acknowledged":true}))
			}
		}
	}
}
#[rstest]
#[tokio::test]
async fn acknowledgement_keeps_health_send_and_database_mark_in_order(repository: Repository) {
	let mut cursor = ReceiptCursor::default();
	acknowledge(&repository, &mut cursor).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(
		trace.calls,
		vec![
			"receipts",
			"GET:/v1/health",
			&format!("POST:/v1/operations/{}/ack", Uuid::from_u128(1)),
			"mark"
		]
	);
	assert_eq!(trace.marked, vec![Uuid::from_u128(1)]);
	assert_eq!(cursor.0, Uuid::from_u128(1));
}
#[rstest]
#[tokio::test]
async fn a_proven_changed_runner_identity_marks_committed_data_without_contacting_the_old_journal(
	repository: Repository,
) {
	repository.0.lock().unwrap().health = json!({"instance":"replacement"});
	acknowledge(&repository, &mut ReceiptCursor::default())
		.await
		.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.calls, vec!["receipts", "GET:/v1/health", "mark"]);
	assert_eq!(trace.marked.len(), 1);
}
#[rstest]
#[tokio::test]
async fn failed_acknowledgement_keeps_the_database_receipt_retryable_and_advances_the_scan_cursor(
	repository: Repository,
) {
	repository.0.lock().unwrap().ack_failure = true;
	let mut cursor = ReceiptCursor::default();
	acknowledge(&repository, &mut cursor).await.unwrap();
	let trace = repository.0.lock().unwrap();
	assert!(trace.marked.is_empty());
	assert_eq!(cursor.0, Uuid::from_u128(1));
}
#[rstest]
#[case::health("health")]
#[case::mark("mark")]
#[tokio::test]
async fn failed_health_or_mark_preserves_the_error_and_cursor_without_a_false_receipt(
	repository: Repository,
	#[case] failure: &str,
) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.health_failure = failure == "health";
		trace.mark_failure = failure == "mark";
	}
	let mut cursor = ReceiptCursor::default();
	assert!(
		matches!(acknowledge(&repository,&mut cursor).await,Err(Error::External(message)) if message==format!("{failure} failed"))
	);
	let trace = repository.0.lock().unwrap();
	assert!(trace.marked.is_empty());
	assert_eq!(cursor.0, Uuid::from_u128(1));
}
#[rstest]
#[tokio::test]
async fn an_empty_receipt_page_wraps_the_cursor_back_to_nil(repository: Repository) {
	let mut cursor = ReceiptCursor::default();
	acknowledge(&repository, &mut cursor).await.unwrap();
	acknowledge(&repository, &mut cursor).await.unwrap();
	acknowledge(&repository, &mut cursor).await.unwrap();
	assert_eq!(
		repository.0.lock().unwrap().after,
		vec![Uuid::nil(), Uuid::from_u128(1), Uuid::nil()]
	);
}
#[rstest]
#[tokio::test]
async fn receipt_failure_does_not_terminate_a_reconciliation_batch(repository: Repository) {
	repository.0.lock().unwrap().health_failure = true;
	assert!(
		sweep(&repository, &mut ReceiptCursor::default())
			.await
			.is_ok()
	);
	assert!(repository.0.lock().unwrap().marked.is_empty());
}
#[rstest]
#[tokio::test]
async fn failed_operation_reconciliation_does_not_starve_following_ids_or_receipts(
	repository: Repository,
) {
	repository.0.lock().unwrap().active = vec![Uuid::from_u128(2), Uuid::from_u128(3)];
	sweep(&repository, &mut ReceiptCursor::default())
		.await
		.unwrap();
	let trace = repository.0.lock().unwrap();
	assert!(
		trace
			.calls
			.contains(&format!("snapshot:{}", Uuid::from_u128(2)))
	);
	assert!(
		trace
			.calls
			.contains(&format!("snapshot:{}", Uuid::from_u128(3)))
	);
	assert_eq!(trace.marked.len(), 1);
}
#[rstest]
#[tokio::test]
async fn domain_quota_failures_retain_the_same_blocked_publication_disclosure(
	repository: Repository,
) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.active = vec![Uuid::from_u128(2)];
		trace.quota = true;
	}
	sweep(&repository, &mut ReceiptCursor::default())
		.await
		.unwrap();
	let trace = repository.0.lock().unwrap();
	assert_eq!(trace.blocked.len(), 1);
	assert_eq!(trace.blocked[0]["code"], json!("STORAGE_QUOTA"));
	assert_eq!(trace.marked.len(), 1);
}
#[rstest]
#[case::scan("scan")]
#[case::blocked_update("block")]
#[tokio::test]
async fn fatal_scan_and_blocked_update_errors_stop_the_batch_before_receipt_work(
	repository: Repository,
	#[case] failure: &str,
) {
	{
		let mut trace = repository.0.lock().unwrap();
		trace.active_failure = failure == "scan";
		trace.block_failure = failure == "block";
		trace.quota = true;
		trace.active = vec![Uuid::from_u128(2)];
	}
	assert!(
		matches!(sweep(&repository,&mut ReceiptCursor::default()).await,Err(Error::External(message)) if message==format!("{failure} failed"))
	);
	assert!(
		!repository
			.0
			.lock()
			.unwrap()
			.calls
			.contains(&"receipts".into())
	);
}
#[rstest]
#[case::native_conflict(Error::Conflict("STORAGE_QUOTA: tenant".into()),true)]
#[case::domain_conflict(Error::Domain(aidash_domain::Error::Conflict("runner output quota".into())),true)]
#[case::external(Error::External("runner output quota".into()),false)]
#[case::other_conflict(Error::Conflict("AREA_BUSY".into()),false)]
fn publication_disclosure_preserves_conflict_classification_across_the_domain_boundary(
	#[case] error: Error,
	#[case] blocked: bool,
) {
	assert_eq!(blocked_publication(&error).is_some(), blocked);
}
