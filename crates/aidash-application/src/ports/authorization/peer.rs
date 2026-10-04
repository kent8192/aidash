//! Receiver scopes retain one authority transaction across every protected effect.
use crate::Result;
use aidash_domain::{
	Run,
	federation::execution::{
		Description,
		admission::{InspectInput, Record, RemoteExecutionControl},
	},
	generation::remote::Ancestor,
	identity::execution::ExecutionPrincipal,
	policy::{PolicyBundle, Resource},
	registry::{EntityRef, Entry},
	run_state::RunInspection,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait PeerInspectionScope: Send {
	fn node_id(&self) -> &str;
	fn identity(&self) -> ExecutionPrincipal;
	fn bundle(&self) -> &PolicyBundle;
	fn subjects(&self) -> &[String];
	fn push_subject(&mut self, subject: String);
	fn context(&mut self) -> &mut Value;
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn generation(&mut self, source: &str, input: &InspectInput) -> Result<Option<Value>>;
	async fn entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry>;
	fn entry_resource(&self, entry: &Entry) -> Resource;
	async fn active_installation(&mut self, entry: &Entry) -> Result<bool>;
	async fn pinned_installation(&mut self, entry: &Entry) -> Result<()>;
	async fn lineage(&mut self) -> Result<Vec<Ancestor>>;
}
#[async_trait]
pub trait PeerAdmissionScope: PeerInspectionScope + Sized {
	async fn lock_admission(&mut self, source: &str, description: &Description) -> Result<()>;
	async fn legacy_conflict(
		&mut self,
		source: &str,
		description: &Description,
		grant: Uuid,
	) -> Result<bool>;
	async fn existing(&mut self, source: &str, grant: Uuid) -> Result<Option<Uuid>>;
	async fn insert(
		&mut self,
		source: &str,
		grant: Uuid,
		description: &Description,
		proposed: Uuid,
	) -> Result<()>;
	async fn admitted(&mut self, source: &str, grant: Uuid) -> Result<Option<Record>>;
	async fn admission_live(&mut self, expires: DateTime<Utc>) -> Result<bool>;
	async fn receiver_live(&mut self, expires: DateTime<Utc>) -> Result<bool>;
	async fn bind_foreign(
		&mut self,
		description: &Description,
		id: Uuid,
		activate: bool,
	) -> Result<()>;
	async fn required_record(&mut self, id: Uuid) -> Result<Record>;
	async fn require_active(&mut self, description: &Description, id: Uuid) -> Result<()>;
	fn worker(&mut self, durable: bool);
	async fn activation_record(&mut self, id: Uuid) -> Result<Option<Record>>;
	async fn lock_activation(&mut self, source: &str, description: &Description) -> Result<()>;
	async fn insert_run(&mut self, source: &str, id: Uuid, description: &Description)
	-> Result<()>;
	async fn activation_run(&mut self, id: Uuid) -> Result<Option<Run>>;
	async fn accept_message(
		self,
		id: Uuid,
		sender: &str,
		content: &str,
		key: &str,
		limit: usize,
	) -> Result<()>;
	async fn finish(self, result: Result<()>) -> Result<()>;
}
#[async_trait]
pub trait PeerAdmissionRecords: Send + Sync {
	async fn record(&self, id: Uuid) -> Result<Option<Record>>;
	async fn description(&self, id: Uuid) -> Result<Value>;
}
#[async_trait]
pub trait AdmissionMessages: Send + Sync {
	async fn reserve(&self, key: &str, content: &str) -> Result<bool>;
	async fn release(&self, key: &str) -> Result<()>;
	async fn commit(&self, key: &str, content: &str) -> Result<()>;
}
#[async_trait]
pub trait PeerAdmissionRepository: PeerAdmissionRecords {
	type Scope: PeerAdmissionScope;
	fn node_id(&self) -> &str;
	async fn request(&self, source: &str, path: &str, input: &Value) -> Result<Value>;
	async fn mapped(&self, source: &str, tenant: &str, subject: &str) -> Result<Self::Scope>;
	async fn verify_record(&self, id: Uuid, source: &str) -> Result<Option<Record>>;
	async fn status_record(&self, source: &str, grant: Uuid) -> Result<Option<Record>>;
	async fn status_run(&self, id: Uuid, source: &str) -> Result<Option<RunInspection>>;
	async fn leaf_record(&self, source: &str, grant: Uuid, id: Uuid) -> Result<Option<Record>>;
	async fn run(&self, id: Uuid) -> Result<Run>;
	async fn inspect_run(&self, id: Uuid) -> Result<RunInspection>;
	async fn control(&self, id: Uuid, action: &RemoteExecutionControl) -> Result<RunInspection>;
	fn messages(&self, run: &Run) -> Box<dyn AdmissionMessages + '_>;
	async fn message_limit(&self, run: &Run) -> Result<usize>;
	async fn message_recorded(&self, id: Uuid, key: &str, content: &str) -> bool;
	async fn deliver(&self, run: &Run) -> Result<()>;
	fn notify(&self);
}
