use super::*;
use async_trait::async_trait;
use std::sync::Mutex;

struct Repository {
	page: AllocationPage,
	sink_fails: bool,
	ack_fails: bool,
	calls: Mutex<Vec<&'static str>>,
	after: Mutex<Vec<Option<Uuid>>>,
	events: Mutex<Vec<crate::usage::PodTime>>,
}
fn repository() -> Repository {
	let mut receipt: AllocationReceipt = serde_json::from_value(serde_json::json!({
		"protocol":"aidash-allocation/1", "allocation_id":"00000000-0000-0000-0000-000000000011",
		"runner_instance":"00000000-0000-0000-0000-000000000001",
		"tenant":"purged-tenant", "area_id":"00000000-0000-0000-0000-000000000012",
		"operation_id":"00000000-0000-0000-0000-000000000013", "epoch":1,
		"started_at":"2026-10-01T23:59:00Z", "ended_at":"2026-10-02T00:02:01Z",
		"evidence":{"pod_uid":"00000000-0000-0000-0000-000000000011",
			"start_time":"2026-10-01T23:59:00Z",
			"containers":[{"name":"collector","finished_at":"2026-10-02T00:02:01Z"},
						  {"name":"execution","finished_at":"2026-10-02T00:02:00Z"}]},
		"digest":""
	}))
	.unwrap();
	// Fixed independent Python canonical JSON SHA256 fixture, not the validator under test.
	receipt.digest = "a62ed32c8ad21f3d59bb9b36fbd6eb5f825781ddadeb87f141abdda64592dac2".into();
	Repository {
		page: AllocationPage {
			instance: Uuid::from_u128(1),
			receipts: vec![receipt],
			next_after: None,
			pending_evidence: 0,
		},
		sink_fails: false,
		ack_fails: false,
		calls: Mutex::new(vec![]),
		after: Mutex::new(vec![]),
		events: Mutex::new(vec![]),
	}
}
#[async_trait]
impl PodUsageRepository for Repository {
	async fn pending_allocations(&self, after: Option<Uuid>) -> Result<AllocationPage> {
		self.calls.lock().unwrap().push("pending");
		self.after.lock().unwrap().push(after);
		Ok(self.page.clone())
	}
	async fn deliver_allocation(&self, event: crate::usage::PodTime) -> Result<()> {
		self.calls.lock().unwrap().push("sink");
		if self.sink_fails {
			return Err(crate::Error::External("sink unavailable".into()));
		}
		self.events.lock().unwrap().push(event);
		Ok(())
	}
	async fn acknowledge_allocation(&self, _: &AllocationReceipt) -> Result<()> {
		self.calls.lock().unwrap().push("ack");
		if self.ack_fails {
			return Err(crate::Error::External("ack unavailable".into()));
		}
		Ok(())
	}
}
#[tokio::test]
async fn a_revoked_or_purged_tenant_needs_no_authority_to_deliver_a_complete_allocation() {
	let repository = repository();
	sweep(&repository, &mut Cursor::default()).await.unwrap();
	assert_eq!(
		*repository.calls.lock().unwrap(),
		["pending", "sink", "ack"]
	);
	let events = repository.events.lock().unwrap();
	assert_eq!(events[0].allocation, Uuid::from_u128(17));
	assert_eq!(events[0].tenant, "purged-tenant");
	assert_eq!(events[0].receipt_digest, repository.page.receipts[0].digest);
	assert_eq!(
		(events[0].ended_at - events[0].started_at).num_seconds(),
		181
	);
}
#[tokio::test]
async fn a_sink_failure_never_acknowledges_and_an_ack_failure_replays_the_same_event_after_restart()
{
	let mut repository = repository();
	repository.sink_fails = true;
	assert!(sweep(&repository, &mut Cursor::default()).await.is_err());
	assert_eq!(*repository.calls.lock().unwrap(), ["pending", "sink"]);
	repository.sink_fails = false;
	repository.ack_fails = true;
	assert!(sweep(&repository, &mut Cursor::default()).await.is_err());
	repository.ack_fails = false;
	sweep(&repository, &mut Cursor::default()).await.unwrap();
	let events = repository.events.lock().unwrap();
	assert_eq!(events.len(), 2);
	assert_eq!(events[0], events[1]);
}
#[tokio::test]
async fn altered_receipts_or_journal_identity_loss_are_never_delivered_or_acknowledged() {
	for mutation in ["digest", "instance", "time", "uid"] {
		let mut repository = repository();
		let receipt = &mut repository.page.receipts[0];
		match mutation {
			"digest" => receipt.tenant = "another-tenant".into(),
			"instance" => repository.page.instance = Uuid::new_v4(),
			"time" => receipt.ended_at = "2025-01-01T00:00:00Z".into(),
			"uid" => receipt.evidence.pod_uid = Uuid::new_v4(),
			_ => unreachable!(),
		}
		assert!(
			sweep(&repository, &mut Cursor::default()).await.is_err(),
			"{mutation}"
		);
		assert_eq!(*repository.calls.lock().unwrap(), ["pending"], "{mutation}");
	}
}
#[tokio::test]
async fn unavailable_evidence_remains_operationally_visible() {
	let mut repository = repository();
	repository.page.receipts.clear();
	repository.page.pending_evidence = 1;
	assert!(sweep(&repository, &mut Cursor::default()).await.is_err());
	assert_eq!(*repository.calls.lock().unwrap(), ["pending"]);
}

#[tokio::test]
async fn last_pages_wrap_and_empty_pages_still_detect_journal_replacement() {
	let mut repository = repository();
	repository.page.next_after = Some(Uuid::from_u128(17));
	let mut cursor = Cursor::default();
	sweep(&repository, &mut cursor).await.unwrap();
	repository.page.receipts.clear();
	repository.page.next_after = None;
	sweep(&repository, &mut cursor).await.unwrap();
	repository.page.instance = Uuid::new_v4();
	assert!(sweep(&repository, &mut cursor).await.is_err());
	assert_eq!(
		*repository.after.lock().unwrap(),
		[None, Some(Uuid::from_u128(17)), None]
	);
	assert_eq!(repository.events.lock().unwrap().len(), 1);
}
