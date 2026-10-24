//! Inspection keeps current policy, usage, draft sharing and evidence on one audit lease.
use crate::Result;
use aidash_domain::{
	RunMetadata,
	identity::Principal,
	policy::Resource,
	registry::{
		EntityRef, Entry,
		workbench::{
			Draft,
			audit::Registration,
			inspection::{Inspection, TestObservation},
		},
	},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;
#[async_trait]
pub trait InspectionScope: Send {
	fn principal(&self) -> Principal;
	async fn require_inspection(&mut self, reference: &EntityRef) -> Result<()>;
	async fn effective(&mut self, reference: &EntityRef) -> Result<Entry>;
	/// Preserve the 501-row keyset page and the metadata-only Run boundary.
	async fn agent_runs(
		&mut self,
		reference: &EntityRef,
		cursor: Option<(DateTime<Utc>, Uuid)>,
	) -> Result<Vec<RunMetadata>>;
	async fn workspace(&mut self, id: Uuid) -> Result<Resource>;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
	async fn run_visible(&mut self, run: &RunMetadata) -> Result<bool>;
	async fn workspace_title(&mut self, id: Uuid) -> Result<Option<String>>;
	/// Preserve the existing one-registration source bound and order.
	async fn registrations(&mut self, reference: &EntityRef) -> Result<Vec<Registration>>;
	async fn draft(&mut self, id: Uuid) -> Result<Draft>;
	async fn authorize_draft(&mut self, draft: &Draft) -> Result<()>;
	/// Preserve the 101-row overflow sentinel for the 100 visible observations.
	async fn test_observations(
		&mut self,
		draft: Uuid,
		revision: i64,
	) -> Result<Vec<TestObservation>>;
	/// Finish the original audit allocation before delivering the inspection.
	async fn finish(self: Box<Self>, inspection: Inspection) -> Result<Inspection>;
}
#[async_trait]
pub trait InspectionRepository: Send + Sync {
	fn node_id(&self) -> &str;
	async fn begin(&self) -> Result<Box<dyn InspectionScope + '_>>;
}
