use super::*;
use async_trait::async_trait;
use std::sync::Mutex;

struct Repository {
	page: AllocationPage,
	page_size: usize,
	sink_fails: Option<&'static str>,
	ack_fails: Option<Uuid>,
	calls: Mutex<Vec<&'static str>>,
	after: Mutex<Vec<Option<Uuid>>>,
	events: Mutex<Vec<crate::usage::PodTime>>,
	ack_attempts: Mutex<Vec<Uuid>>,
	acknowledged: Mutex<BTreeSet<Uuid>>,
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
		page_size: 16,
		sink_fails: None,
		ack_fails: None,
		calls: Mutex::new(vec![]),
		after: Mutex::new(vec![]),
		events: Mutex::new(vec![]),
		ack_attempts: Mutex::new(vec![]),
		acknowledged: Mutex::new(BTreeSet::new()),
	}
}

fn later_receipt(allocation: u128) -> AllocationReceipt {
	let mut receipt = repository().page.receipts.remove(0);
	receipt.allocation_id = Uuid::from_u128(allocation);
	receipt.evidence.pod_uid = receipt.allocation_id;
	receipt.tenant = "later-tenant".into();
	// Independent Python canonical JSON SHA256 fixtures for the changed identity.
	receipt.digest = match allocation {
		18 => "403707b3082bbacd177327d2e6d9fc4ee48456e1dd46eafe5c693c9543cb968a",
		19 => "863635f73d4e13fe5707ec27fd78bba2a5f78c8ea2f1af49dfb51c321b930c5e",
		_ => unreachable!(),
	}
	.into();
	receipt
}

#[async_trait]
impl PodUsageRepository for Repository {
	async fn pending_allocations(&self, after: Option<Uuid>) -> Result<AllocationPage> {
		self.calls.lock().unwrap().push("pending");
		self.after.lock().unwrap().push(after);
		let acknowledged = self.acknowledged.lock().unwrap();
		let mut page = self.page.clone();
		page.receipts.retain(|receipt| {
			!acknowledged.contains(&receipt.allocation_id)
				&& after.is_none_or(|after| receipt.allocation_id > after)
		});
		page.receipts.truncate(self.page_size);
		if page.receipts.len() == self.page_size {
			page.next_after = page.receipts.last().map(|receipt| receipt.allocation_id);
		}
		Ok(page)
	}
	async fn deliver_allocation(&self, event: crate::usage::PodTime) -> Result<()> {
		self.calls.lock().unwrap().push("sink");
		if self.sink_fails == Some(event.tenant.as_str()) {
			return Err(crate::Error::External("sink unavailable".into()));
		}
		self.events.lock().unwrap().push(event);
		Ok(())
	}
	async fn acknowledge_allocation(&self, receipt: &AllocationReceipt) -> Result<()> {
		self.calls.lock().unwrap().push("ack");
		self.ack_attempts
			.lock()
			.unwrap()
			.push(receipt.allocation_id);
		if self.ack_fails == Some(receipt.allocation_id) {
			return Err(crate::Error::External("ack unavailable".into()));
		}
		self.acknowledged
			.lock()
			.unwrap()
			.insert(receipt.allocation_id);
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
#[rstest::rstest]
#[case::delivery(true)]
#[case::acknowledgement(false)]
#[tokio::test]
async fn failed_receipts_do_not_block_other_tenants_or_pages_and_retry_after_wrap(
	#[case] sink_failure: bool,
) {
	let [failed, same_page, next_page] = [17, 18, 19].map(Uuid::from_u128);
	let mut repository = repository();
	repository.page_size = 2;
	repository
		.page
		.receipts
		.extend([later_receipt(18), later_receipt(19)]);
	if sink_failure {
		repository.sink_fails = Some("purged-tenant");
	} else {
		repository.ack_fails = Some(failed);
	}
	let mut cursor = Cursor::default();
	let error = sweep(&repository, &mut cursor).await.unwrap_err();
	assert!(error.to_string().contains(if sink_failure {
		"sink unavailable"
	} else {
		"ack unavailable"
	}));
	assert_eq!(
		*repository.acknowledged.lock().unwrap(),
		BTreeSet::from([same_page]),
		"a failed receipt must not block the later Tenant in its page"
	);
	sweep(&repository, &mut cursor).await.unwrap();
	assert_eq!(
		*repository.acknowledged.lock().unwrap(),
		BTreeSet::from([same_page, next_page]),
		"pagination must progress while the failed receipt remains unacknowledged"
	);

	// The last page wraps to the retained failure, which remains visible.
	assert!(sweep(&repository, &mut cursor).await.is_err());
	assert_eq!(
		*repository.after.lock().unwrap(),
		[None, Some(same_page), None]
	);
	assert_eq!(
		*repository.acknowledged.lock().unwrap(),
		BTreeSet::from([same_page, next_page])
	);
	if sink_failure {
		assert_eq!(
			*repository.ack_attempts.lock().unwrap(),
			[same_page, next_page],
			"a rejected delivery must never be acknowledged"
		);
	} else {
		// Recovering ACK after a native restart replays the same idempotency payload.
		repository.ack_fails = None;
		sweep(&repository, &mut Cursor::default()).await.unwrap();
		assert_eq!(
			*repository.acknowledged.lock().unwrap(),
			BTreeSet::from([failed, same_page, next_page])
		);
		let events = repository.events.lock().unwrap();
		let retries: Vec<_> = events
			.iter()
			.filter(|event| event.allocation == failed)
			.collect();
		assert_eq!(retries.len(), 3);
		assert_eq!(retries[0], retries[1]);
		assert_eq!(retries[0], retries[2]);
	}
}
#[tokio::test]
async fn altered_receipts_or_journal_identity_loss_are_never_delivered_or_acknowledged() {
	for mutation in ["digest", "instance", "time", "uid", "order"] {
		let mut repository = repository();
		repository.page.receipts.push(later_receipt(18));
		let receipt = &mut repository.page.receipts[1];
		match mutation {
			"digest" => receipt.tenant = "another-tenant".into(),
			"instance" => repository.page.instance = Uuid::new_v4(),
			"time" => receipt.ended_at = "2025-01-01T00:00:00Z".into(),
			"uid" => receipt.evidence.pod_uid = Uuid::new_v4(),
			"order" => repository.page.receipts.swap(0, 1),
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
