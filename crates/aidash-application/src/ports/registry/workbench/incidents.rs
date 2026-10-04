//! Incident commands retain current identity and row locks until the record and history commit together.
use crate::Result;
use aidash_domain::{
	identity::Principal,
	policy::{Decision, Evaluation},
	registry::{
		EntityRef, Entry,
		workbench::incident::{Incident, IncidentEvent},
	},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait IncidentScope: Send {
	fn principal(&self) -> Principal;
	async fn lock_identity(&mut self) -> Result<()>;
	async fn evaluate(&mut self, tenant: &str, input: &Evaluation) -> Result<Decision>;
	async fn require_inspection(&mut self, reference: &EntityRef) -> Result<()>;
	async fn effective(&mut self, reference: &EntityRef) -> Result<Entry>;
	async fn target_enabled(&mut self, tenant: &str, subject: &str) -> Result<()>;
	/// Preserve exclusive incident locks for updates and event history, unlocked projections for get.
	async fn read(&mut self, id: Uuid, lock: bool) -> Result<Incident>;
	/// Preserve the 100-row keyset page and tenant predicate before visibility filtering.
	async fn page(
		&mut self,
		reference: &EntityRef,
		tenant: Option<&str>,
		cursor: Option<(DateTime<Utc>, Uuid)>,
	) -> Result<Vec<Incident>>;
	async fn insert(&mut self, incident: &Incident) -> Result<()>;
	/// Increment the revision and update the timestamp atomically under the existing incident lock.
	async fn save(&mut self, incident: &Incident) -> Result<()>;
	async fn record_event(&mut self, id: Uuid, actor: &str, change: Value) -> Result<()>;
	async fn events(&mut self, id: Uuid, limit: usize) -> Result<Vec<IncidentEvent>>;
	async fn evidence_days(&mut self, tenant: &str) -> Result<i32>;
	async fn commit(self: Box<Self>) -> Result<()>;
}
#[async_trait]
pub trait IncidentRepository: Send + Sync {
	fn principal(&self) -> Principal;
	async fn begin(&self) -> Result<Box<dyn IncidentScope + '_>>;
}
#[async_trait]
pub trait IncidentRetentionScope: Send {
	/// Retain the 100-row FOR UPDATE SKIP LOCKED batch and current expiry predicates.
	async fn expired(&mut self) -> Result<Vec<Incident>>;
	async fn discard(&mut self, id: Uuid, evidence: Value) -> Result<()>;
	async fn commit(self: Box<Self>) -> Result<()>;
}
#[async_trait]
pub trait IncidentRetentionRepository: Send + Sync {
	async fn begin(&self) -> Result<Box<dyn IncidentRetentionScope + '_>>;
}
