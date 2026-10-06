//! Authority operations borrow one locked policy and credential transaction.
use crate::Result;
use aidash_domain::{
	RunMetadata, Task, identity::execution::ExecutionPrincipal, policy::Resource,
	transactions::authority::SourceAdmission,
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait TransactionAuthorityScope: Send {
	fn identity(&self) -> ExecutionPrincipal;
	fn source_node(&self) -> Option<&str>;
	fn subjects(&self) -> &[String];
	fn set_subjects(&mut self, subjects: Vec<String>);
	fn resource(&self, kind: &str, id: Uuid, attributes: Value) -> Resource;
	fn qualified_resource(&self, kind: &str, node: &str, id: Uuid) -> Resource;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn inherit_task(&mut self, task: Uuid) -> Result<()>;
	async fn inherit_local_run(&mut self, run: &RunMetadata) -> Result<()>;
	async fn source_admission(
		&mut self,
		run: &RunMetadata,
		coordinator: &str,
	) -> Result<Option<SourceAdmission>>;
	async fn run(&mut self, id: Uuid) -> Result<RunMetadata>;
	async fn workspace(&mut self, id: Uuid) -> Result<Resource>;
	async fn task(&mut self, id: Uuid) -> Result<Task>;
	async fn task_resource(&mut self, task: &Task) -> Result<Resource>;
	async fn artifact_creation_resource(&mut self, task: Uuid, owner: &str) -> Result<Resource>;
}

pub mod admission;
pub mod authority;
pub mod coordination;
pub mod management;
pub mod mutation;
pub mod participation;
