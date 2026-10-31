use super::*;
use crate::ports::research::RevocationScope;
use aidash_domain::retention::RecordedDecider;
use async_trait::async_trait;
use std::sync::{
	Mutex,
	atomic::{AtomicBool, Ordering},
};

/// One Run, an optional permission and the revocations recorded so far.
struct Runs {
	decider: Decider,
	run: Uuid,
	permitted: bool,
	recorded: Mutex<Option<EvidenceRevocation>>,
	committed: AtomicBool,
	notified: AtomicBool,
}

impl Runs {
	fn new(decider: Decider, permitted: bool) -> Self {
		Self {
			decider,
			run: Uuid::new_v4(),
			permitted,
			recorded: Mutex::new(None),
			committed: AtomicBool::new(false),
			notified: AtomicBool::new(false),
		}
	}
}

struct Scope<'a>(&'a Runs);

#[async_trait]
impl RevocationScope for Scope<'_> {
	fn decider(&self) -> Decider {
		self.0.decider.clone()
	}
	async fn require(&mut self, run: Uuid, action: &str) -> Result<()> {
		assert_eq!(action, "research.revoke");
		if run == self.0.run && self.0.permitted {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}
	async fn tenant(&mut self, _: Uuid) -> Result<Option<String>> {
		Ok(Some("acme".into()))
	}
	async fn revoke(&mut self, revocation: &EvidenceRevocation) -> Result<EvidenceRevocation> {
		let mut recorded = self.0.recorded.lock().unwrap();
		Ok(recorded.get_or_insert_with(|| revocation.clone()).clone())
	}
	async fn finish(
		self: Box<Self>,
		result: Result<EvidenceRevocation>,
	) -> Result<EvidenceRevocation> {
		self.0.committed.store(result.is_ok(), Ordering::SeqCst);
		result
	}
}

struct Records<'a>(&'a Runs);

#[async_trait]
impl crate::ports::research::RevocationRecordScope for Records<'_> {
	fn decider(&self) -> Decider {
		self.0.decider.clone()
	}
	async fn permits(&mut self, _: &str, action: &str) -> Result<bool> {
		assert_eq!(action, "research.revoke");
		Ok(self.0.permitted)
	}
	async fn permits_run(&mut self, run: Uuid, action: &str) -> Result<bool> {
		assert_eq!(action, "research.revoke");
		Ok(run == self.0.run && self.0.permitted)
	}
	async fn record(&mut self, run: Uuid) -> Result<Option<EvidenceRevocation>> {
		Ok(self
			.0
			.recorded
			.lock()
			.unwrap()
			.clone()
			.filter(|record| record.run_id == run))
	}
	async fn records(
		&mut self,
		tenant: &str,
		offset: u64,
		limit: u32,
	) -> Result<Vec<EvidenceRevocation>> {
		Ok(self
			.0
			.recorded
			.lock()
			.unwrap()
			.iter()
			.filter(|record| record.tenant.as_deref() == Some(tenant))
			.skip(offset as usize)
			.take(limit as usize)
			.cloned()
			.collect())
	}
	async fn finish(self: Box<Self>, _: bool) -> Result<()> {
		Ok(())
	}
}

#[async_trait]
impl RevocationRepository for Runs {
	async fn begin(&self) -> Result<Box<dyn crate::ports::research::RevocationScope + '_>> {
		Ok(Box::new(Scope(self)))
	}
	async fn begin_records(
		&self,
	) -> Result<Box<dyn crate::ports::research::RevocationRecordScope + '_>> {
		Ok(Box::new(Records(self)))
	}
	fn notify(&self) {
		self.notified.store(true, Ordering::SeqCst);
	}
}

fn subject() -> Decider {
	Decider::Subject {
		tenant: "acme".into(),
		subject: "user:ada".into(),
	}
}

#[tokio::test]
async fn a_permitted_subject_revokes_and_workers_are_woken() {
	let runs = Runs::new(subject(), true);
	let now = Utc::now();
	let revoked = revoke(&runs, runs.run, " Removal request ", None, now)
		.await
		.unwrap();
	assert_eq!(revoked.run_id, runs.run);
	assert_eq!(revoked.tenant.as_deref(), Some("acme"));
	assert_eq!(revoked.revoked_by, subject().into());
	assert_eq!(revoked.reason, "Removal request");
	assert_eq!(revoked.revoked_at, now);
	assert!(runs.committed.load(Ordering::SeqCst));
	assert!(runs.notified.load(Ordering::SeqCst));
}

#[tokio::test]
async fn revocation_is_irreversible_and_keeps_its_first_record() {
	let runs = Runs::new(subject(), true);
	let first = revoke(&runs, runs.run, "first", None, Utc::now())
		.await
		.unwrap();
	let second = revoke(&runs, runs.run, "second", None, Utc::now())
		.await
		.unwrap();
	assert_eq!(second, first);
	assert_eq!(second.reason, "first");
}

#[tokio::test]
async fn a_subject_without_permission_learns_nothing() {
	let runs = Runs::new(subject(), false);
	for run in [runs.run, Uuid::new_v4()] {
		assert!(matches!(
			revoke(&runs, run, "reason", None, Utc::now()).await,
			Err(Error::Forbidden)
		));
	}
	assert!(runs.recorded.lock().unwrap().is_none());
}

#[tokio::test]
async fn a_reason_is_required_before_anything_is_opened() {
	let runs = Runs::new(subject(), true);
	assert!(matches!(
		revoke(&runs, runs.run, "  ", None, Utc::now()).await,
		Err(Error::Domain(_))
	));
	assert!(!runs.notified.load(Ordering::SeqCst));
}

#[tokio::test]
async fn revocation_records_are_read_by_revokers() {
	let runs = Runs::new(subject(), true);
	revoke(&runs, runs.run, "removal request", None, Utc::now())
		.await
		.unwrap();
	assert_eq!(records(&runs, "acme", 0, 50).await.unwrap().len(), 1);
	assert!(matches!(
		records(&runs, "other", 0, 50).await,
		Err(Error::Forbidden)
	));

	let reader = Runs::new(subject(), false);
	assert!(matches!(
		records(&reader, "acme", 0, 50).await,
		Err(Error::Forbidden)
	));
}

#[tokio::test]
async fn a_run_of_another_tenant_than_the_one_named_is_unavailable() {
	let runs = Runs::new(subject(), true);
	assert!(matches!(
		revoke(&runs, runs.run, "reason", Some("other"), Utc::now()).await,
		Err(Error::NotFound(_))
	));
	assert!(runs.recorded.lock().unwrap().is_none());
	assert!(!runs.committed.load(Ordering::SeqCst));
	let revoked = revoke(&runs, runs.run, "reason", Some("acme"), Utc::now())
		.await
		.unwrap();
	assert_eq!(revoked.tenant.as_deref(), Some("acme"));
}

#[tokio::test]
async fn every_reader_learns_when_but_only_revokers_read_the_record() {
	let runs = Runs::new(subject(), true);
	assert_eq!(
		standing(&runs, runs.run).await.unwrap(),
		Standing {
			revoked_at: None,
			may_revoke: true,
			may_read_record: true,
		}
	);
	assert!(matches!(
		record(&runs, runs.run).await,
		Err(Error::NotFound(_))
	));
	let revoked = revoke(&runs, runs.run, "removal request", None, Utc::now())
		.await
		.unwrap();
	assert_eq!(
		standing(&runs, runs.run).await.unwrap().revoked_at,
		Some(revoked.revoked_at)
	);
	assert_eq!(record(&runs, runs.run).await.unwrap(), revoked);

	let reader = Runs::new(subject(), false);
	*reader.recorded.lock().unwrap() = Some(revoked.clone());
	let reader_standing = Standing {
		revoked_at: Some(revoked.revoked_at),
		may_revoke: false,
		may_read_record: false,
	};
	// The reader's own Run was not revoked; the other one was.
	let unrevoked = Standing {
		revoked_at: None,
		..reader_standing.clone()
	};
	assert_eq!(standing(&reader, reader.run).await.unwrap(), unrevoked);
	assert_eq!(
		standing(&reader, revoked.run_id).await.unwrap(),
		reader_standing
	);
	assert!(matches!(
		record(&reader, revoked.run_id).await,
		Err(Error::Forbidden)
	));
}

#[tokio::test]
async fn a_subject_reads_no_record_of_another_tenants_run() {
	let runs = Runs::new(subject(), true);
	*runs.recorded.lock().unwrap() = Some(EvidenceRevocation {
		run_id: runs.run,
		tenant: Some("other".into()),
		revoked_by: RecordedDecider::Subject {
			tenant: "other".into(),
			subject: "user:eve".into(),
		},
		reason: "agreement ended".into(),
		revoked_at: Utc::now(),
		removed: Removed::default(),
	});
	assert!(matches!(
		record(&runs, runs.run).await,
		Err(Error::NotFound(_))
	));
}

#[tokio::test]
async fn the_records_audience_is_told_before_it_asks() {
	assert!(
		may_read_records(&Runs::new(subject(), true), "acme")
			.await
			.unwrap()
	);
	assert!(
		!may_read_records(&Runs::new(subject(), true), "other")
			.await
			.unwrap()
	);
	assert!(
		!may_read_records(&Runs::new(subject(), false), "acme")
			.await
			.unwrap()
	);
}
