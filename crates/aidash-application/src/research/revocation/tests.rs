use super::*;
use crate::ports::research::RevocationScope;
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
	async fn run_exists(&mut self, run: Uuid) -> Result<bool> {
		Ok(run == self.0.run)
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
	async fn require(&mut self, _: &str, action: &str) -> Result<()> {
		assert_eq!(action, "research.revoke");
		match (&self.0.decider, self.0.permitted) {
			(Decider::DeploymentAdministrator { .. }, _) | (_, true) => Ok(()),
			_ => Err(Error::Forbidden),
		}
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
	let revoked = revoke(&runs, runs.run, " Removal request ", now)
		.await
		.unwrap();
	assert_eq!(revoked.run_id, runs.run);
	assert_eq!(revoked.tenant.as_deref(), Some("acme"));
	assert_eq!(revoked.revoked_by, subject());
	assert_eq!(revoked.reason, "Removal request");
	assert_eq!(revoked.revoked_at, now);
	assert!(runs.committed.load(Ordering::SeqCst));
	assert!(runs.notified.load(Ordering::SeqCst));
}

#[tokio::test]
async fn revocation_is_irreversible_and_keeps_its_first_record() {
	let runs = Runs::new(subject(), true);
	let first = revoke(&runs, runs.run, "first", Utc::now()).await.unwrap();
	let second = revoke(&runs, runs.run, "second", Utc::now()).await.unwrap();
	assert_eq!(second, first);
	assert_eq!(second.reason, "first");
}

#[tokio::test]
async fn a_subject_without_permission_learns_nothing() {
	let runs = Runs::new(subject(), false);
	for run in [runs.run, Uuid::new_v4()] {
		assert!(matches!(
			revoke(&runs, run, "reason", Utc::now()).await,
			Err(Error::Forbidden)
		));
	}
	assert!(runs.recorded.lock().unwrap().is_none());
}

#[tokio::test]
async fn a_deployment_administrator_needs_no_tenant_policy() {
	let runs = Runs::new(Decider::DeploymentAdministrator { identity: None }, false);
	let revoked = revoke(&runs, runs.run, "agreement ended", Utc::now())
		.await
		.unwrap();
	assert_eq!(
		revoked.revoked_by,
		Decider::DeploymentAdministrator { identity: None }
	);
	assert!(matches!(
		revoke(&runs, Uuid::new_v4(), "reason", Utc::now()).await,
		Err(Error::NotFound(_))
	));
}

#[tokio::test]
async fn a_reason_is_required_before_anything_is_opened() {
	let runs = Runs::new(subject(), true);
	assert!(matches!(
		revoke(&runs, runs.run, "  ", Utc::now()).await,
		Err(Error::Domain(_))
	));
	assert!(!runs.notified.load(Ordering::SeqCst));
}

#[tokio::test]
async fn revocation_records_are_read_by_revokers_and_deployment_administrators() {
	let runs = Runs::new(subject(), true);
	revoke(&runs, runs.run, "removal request", Utc::now())
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
	let administrator = Runs::new(Decider::DeploymentAdministrator { identity: None }, false);
	assert!(
		records(&administrator, "other", 0, 50)
			.await
			.unwrap()
			.is_empty()
	);
}
