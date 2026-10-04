//! Audit queries preserve inspection, draft sharing and catalog policy on one native transaction.
use crate::Result;
use aidash_domain::{
	identity::Principal,
	policy::{Decision, Evaluation},
	registry::{
		EntityRef,
		workbench::{
			Draft,
			audit::{CatalogChange, IncidentChange, Registration, TestRecord},
		},
	},
};
use async_trait::async_trait;
use uuid::Uuid;
#[async_trait]
pub trait AuditScope: Send {
	async fn require_inspection(&mut self, reference: &EntityRef) -> Result<()>;
	/// Preserve the existing 100-registration source bound and order.
	async fn registrations(&mut self, reference: &EntityRef) -> Result<Vec<Registration>>;
	async fn draft(&mut self, id: Uuid) -> Result<Draft>;
	async fn authorize_draft(&mut self, draft: &Draft) -> Result<()>;
	/// Preserve the 100-session bound for each authored draft revision.
	async fn test_records(&mut self, draft: Uuid, revision: i64) -> Result<Vec<TestRecord>>;
	async fn evaluate(&mut self, tenant: &str, input: &Evaluation) -> Result<Decision>;
	async fn catalog_history(
		&mut self,
		tenant: &str,
		reference: &EntityRef,
	) -> Result<Vec<CatalogChange>>;
	async fn commit(self: Box<Self>) -> Result<()>;
}
#[async_trait]
pub trait AuditRepository: Send + Sync {
	fn principal(&self) -> Principal;
	async fn begin(&self) -> Result<Box<dyn AuditScope + '_>>;
	/// This is a separately authorized incident read after the audit transaction has committed.
	async fn incidents(&self, reference: &EntityRef) -> Result<Vec<Uuid>>;
	async fn incident_events(&self, id: Uuid) -> Result<Vec<IncidentChange>>;
}
