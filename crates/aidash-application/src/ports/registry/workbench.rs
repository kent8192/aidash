//! Draft authority ports borrow the caller's transaction and current credential.
use super::{DefinitionLookup, DefinitionWriter};
use crate::Result;
use aidash_domain::{
	identity::Principal,
	policy::{Decision, Evaluation, PolicyBundle},
};
use async_trait::async_trait;
use uuid::Uuid;
#[async_trait]
pub trait DraftAuthority: DefinitionLookup {
	fn principal(&self) -> Principal;
	async fn lock_identity(&mut self) -> Result<()>;
	async fn share(&mut self, draft: Uuid, subject: &str) -> Result<Option<(bool, String)>>;
	async fn evaluate(&mut self, tenant: &str, evaluation: &Evaluation) -> Result<Decision>;
	async fn bundle(&mut self, tenant: &str) -> Result<PolicyBundle>;
}

use aidash_domain::registry::{
	Entry,
	workbench::{Draft, ShareRecord},
};
use chrono::{DateTime, Utc};
use serde_json::Value;
/// The scope retains draft row locks, current identity and atomic writes until commit or drop.
#[async_trait]
pub trait DraftScope: DraftAuthority + Sized {
	async fn read(&mut self, id: Uuid, lock: bool) -> Result<Draft>;
	async fn insert(&mut self, draft: &Draft, managed_id: &str) -> Result<Draft>;
	async fn save_content(
		&mut self,
		id: Uuid,
		entry: Value,
		documents: Value,
		notes: &str,
	) -> Result<Draft>;
	async fn page(
		&mut self,
		tenant: Option<&str>,
		cursor: Option<(DateTime<Utc>, Uuid)>,
	) -> Result<Vec<Draft>>;
	async fn managed(&mut self, agent: &str) -> Result<bool>;
	async fn append_event(&mut self, kind: &str, payload: Value) -> Result<()>;
	async fn shares(&mut self, draft: Uuid) -> Result<Vec<ShareRecord>>;
	async fn save_share(
		&mut self,
		draft: Uuid,
		subject: &str,
		can_edit: bool,
		digest: &str,
	) -> Result<()>;
	async fn remove_share(&mut self, draft: Uuid, subject: &str) -> Result<()>;
	async fn transfer(&mut self, id: Uuid, owner: &str) -> Result<()>;
	async fn archive(&mut self, id: Uuid, archived: bool) -> Result<()>;
	async fn commit(self) -> Result<()>;
}
#[async_trait]
pub trait DraftRepository: Send + Sync {
	type Scope: DraftScope;
	fn principal(&self) -> Principal;
	fn node_id(&self) -> &str;
	async fn original_entry(&self, id: &str, version: &str) -> Result<Entry>;
	async fn original_documents(&self, entry: &Entry) -> Result<Value>;
	async fn begin(&self) -> Result<Self::Scope>;
}

use aidash_domain::registry::workbench::{RegistrationEvidence, RegistrationRecord};
/// Publication holds the authored revision through evidence, immutable content and outbox writes.
#[async_trait]
pub trait PublicationScope: DraftScope + DefinitionWriter {
	async fn registrations(&mut self, draft: Uuid, agent: &str) -> Result<Vec<RegistrationRecord>>;
	async fn registered_evidence(&mut self, entry: &Entry) -> Result<Option<RegistrationEvidence>>;
	async fn completed_test(&mut self, draft: &Draft) -> Result<bool>;
	async fn insert_documents(&mut self, entry: &Entry, documents: Value) -> Result<()>;
	async fn record_registration(
		&mut self,
		draft: &Draft,
		entry: &Entry,
		actor: &str,
		behavioral_tested: bool,
	) -> Result<()>;
}

pub mod audit;

pub mod permissions;

pub mod inspection;

pub mod incidents;

pub mod report;

pub mod profile;

pub mod sandbox;
