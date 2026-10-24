//! What a Deployment reports about the work its Tenants cause, and the gate
//! that lets a build refuse new work. Community only emits and asks: both
//! default to doing nothing and admitting everything, and a build that
//! meters or charges for usage supplies the other side.
use crate::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// One complete Execution Pod allocation, identified by its provider Pod UID.
/// Retries retain the same immutable receipt digest and interval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodTime {
	pub allocation: Uuid,
	pub receipt_digest: String,
	pub tenant: String,
	pub started_at: DateTime<Utc>,
	pub ended_at: DateTime<Utc>,
}

/// The bytes of the files Aidash stores for one Tenant when they were counted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageSample {
	pub tenant: String,
	pub sampled_at: DateTime<Utc>,
	pub bytes: u64,
}

#[async_trait]
pub trait UsageEvents: Send + Sync {
	async fn pod_time(&self, event: PodTime) -> Result<()>;
	/// One daily sampling pass, so that an adapter can store it as one unit.
	async fn storage_sampled(&self, samples: Vec<StorageSample>) -> Result<()>;
}

/// Reports nowhere.
pub struct NoUsage;

#[async_trait]
impl UsageEvents for NoUsage {
	async fn pod_time(&self, _: PodTime) -> Result<()> {
		Ok(())
	}

	async fn storage_sampled(&self, _: Vec<StorageSample>) -> Result<()> {
		Ok(())
	}
}

/// What is about to begin in a Tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Work {
	/// A new Run.
	Run,
	/// A Sandboxed tool, which needs an Execution Pod.
	SandboxedTool,
	/// An additional person joining the Tenant by approval.
	Member,
}

/// Why a build refused `Work`. The code is stable and shown to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
	pub code: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
	Admit,
	Refuse(Refusal),
}

impl Verdict {
	pub fn refuse(code: impl Into<String>) -> Self {
		Self::Refuse(Refusal { code: code.into() })
	}
}

/// Decides whether new work may begin. A decision uses only state the
/// Deployment already holds, so a slow or failing outside service never
/// stops a running Tenant from starting work.
#[async_trait]
pub trait Admission: Send + Sync {
	async fn admit(&self, tenant: &str, work: Work) -> Result<Verdict>;
}

/// Admits everything.
pub struct AlwaysAdmit;

#[async_trait]
impl Admission for AlwaysAdmit {
	async fn admit(&self, _: &str, _: Work) -> Result<Verdict> {
		Ok(Verdict::Admit)
	}
}

/// Admit `work` or fail with the build's refusal.
pub async fn require(admission: &dyn Admission, tenant: &str, work: Work) -> Result<()> {
	match admission.admit(tenant, work).await? {
		Verdict::Admit => Ok(()),
		Verdict::Refuse(refusal) => Err(crate::Error::Refused(refusal.code)),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn the_default_gate_admits_every_kind_of_work() {
		for work in [Work::Run, Work::SandboxedTool, Work::Member] {
			require(&AlwaysAdmit, "acme", work).await.unwrap();
		}
	}

	struct Refuses;

	#[async_trait]
	impl Admission for Refuses {
		async fn admit(&self, _: &str, _: Work) -> Result<Verdict> {
			Ok(Verdict::refuse("suspended"))
		}
	}

	#[tokio::test]
	async fn a_refusal_carries_its_code() {
		let error = require(&Refuses, "acme", Work::Run).await.unwrap_err();
		assert!(matches!(error, crate::Error::Refused(code) if code == "suspended"));
	}

	#[tokio::test]
	async fn the_default_sink_accepts_every_event() {
		let now = Utc::now();
		NoUsage
			.pod_time(PodTime {
				allocation: Uuid::nil(),
				receipt_digest: "a".repeat(64),
				tenant: "acme".into(),
				started_at: now,
				ended_at: now,
			})
			.await
			.unwrap();
		NoUsage.storage_sampled(vec![]).await.unwrap();
	}
}
