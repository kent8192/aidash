//! Current source and viewer authority share one protected database transaction.
use crate::{Result, authorization::Snapshot};
use aidash_domain::{
	RunMetadata,
	federation::execution::Description,
	policy::Resource,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone)]
pub struct AdmissionEvidence {
	pub credential_id: Uuid,
	pub subjects: Vec<String>,
	pub description: Description,
}

/// Drop restores the viewer's original authority even on cancellation or error.
/// The temporary terminal-subject exception belongs only to this read lease;
/// it must never enable a stored credential, catalog row or execution subject.
#[async_trait]
pub trait ForeignSourceAuthority: Send {
	fn catalog_resource(&self, entry: &Entry) -> Resource;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
	async fn catalog_entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry>;
}

#[async_trait]
pub trait ForeignRunReadScope: Send {
	fn node(&self) -> &str;
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
	async fn admission(&mut self, run: &RunMetadata) -> Result<Option<AdmissionEvidence>>;
	async fn mapped_credential(
		&mut self,
		run: &RunMetadata,
		description: &Description,
	) -> Result<Option<Uuid>>;
	async fn source_snapshot(&mut self, credential: Uuid, root: &str) -> Result<Snapshot>;
	/// Retain the exact terminal generation, credential, foreign intent and
	/// catalog retirement revision fence, with shared row locks until completion.
	async fn retired_definition(
		&mut self,
		run: &RunMetadata,
		description: &Description,
		credential: Uuid,
		terminal_statuses: &[&str],
	) -> Result<Option<Entry>>;
	fn source_authority(
		&mut self,
		snapshot: Snapshot,
		subjects: Vec<String>,
	) -> Box<dyn ForeignSourceAuthority + '_>;
}
