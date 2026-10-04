//! Admission effects borrow one current native authority transaction.
use crate::Result;
use aidash_domain::capabilities::operations::{AdmissionLimits, AreaSnapshot, MountedFile};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
pub struct AcceptedOperation {
	pub id: Uuid,
	pub key: String,
	pub digest: String,
	pub kind: String,
	pub epoch: i64,
	pub input: Value,
}
#[async_trait]
pub trait OperationAdmissionScope<O: Send>: Send {
	fn limits(&self) -> AdmissionLimits;
	fn area(&self) -> AreaSnapshot;
	async fn previous(&mut self, key: &str) -> Result<Option<(O, String)>>;
	async fn result(&mut self, operation: &O, offset: usize) -> Result<Value>;
	async fn active_run(&mut self) -> Result<Option<Uuid>>;
	async fn require_write(&mut self) -> Result<()>;
	async fn request_files(
		&mut self,
		run: Uuid,
		package_files: Vec<MountedFile>,
	) -> Result<Vec<MountedFile>>;
	async fn verified_health(&mut self, python: bool) -> Result<()>;
	async fn release_python(&mut self, reason: &str) -> Result<()>;
	fn operation_id(&self) -> Uuid;
	async fn set_running(&mut self, epoch: i64) -> Result<()>;
	async fn accept(&mut self, operation: AcceptedOperation) -> Result<O>;
}

/// The loaded operation remains under the same native row lock through control and projection.
#[async_trait]
pub trait OperationControlScope: Send {
	fn area_id(&self) -> Uuid;
	fn run_id(&self) -> Uuid;
	fn principal(&self) -> &str;
	async fn load(
		&mut self,
		id: Uuid,
	) -> Result<aidash_domain::capabilities::operations::OperationState>;
	fn apply_cancellation(
		&mut self,
		change: aidash_domain::capabilities::operations::Cancellation,
	) -> Result<()>;
	async fn activate_area(&mut self) -> Result<()>;
	async fn complete_python_without_writer(&mut self) -> Result<()>;
	async fn persist(&mut self) -> Result<()>;
	async fn project(&mut self, offset: usize) -> Result<Value>;
}

pub mod withdrawal;

pub mod runner;

pub mod reconciliation;

pub mod processing;

pub mod outbound;
