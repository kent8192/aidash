//! Execution authority reads borrow the existing policy/credential transaction.
use crate::Result;
use aidash_domain::{
	identity::execution::{ExecutionGrant, ExecutionPrincipal, TaskOrigin},
	policy::Resource,
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

#[async_trait]
pub trait ExecutionGrantRepository: Send + Sync {
	fn node_id(&self) -> &str;
	async fn grant(&self, run: Uuid) -> Result<Option<ExecutionGrant>>;
	async fn require_legacy_remote_task(&self, home: &str, task: Uuid) -> Result<()>;
	async fn require_legacy_execution(&self, workspace: Uuid) -> Result<()>;
	async fn require_legacy_agent(&self, agent: &str, version: &str) -> Result<()>;
}

#[async_trait]
pub trait ExecutionGrantSession: Send {
	fn identity(&self) -> ExecutionPrincipal;
	fn subjects(&self) -> &[String];
	fn set_subjects(&mut self, subjects: Vec<String>);
	fn select_worker(&mut self, run: Uuid, durable_audit: Option<bool>);
	/// Refresh policy and credential before locking the current grant.
	async fn refresh(&mut self, run: Uuid) -> Result<()>;
	/// Both grant reads hold SHARE locks; required reads preserve missing-row errors.
	async fn required_grant(&mut self, run: Uuid) -> Result<ExecutionGrant>;
	async fn optional_grant(&mut self, run: Uuid) -> Result<Option<ExecutionGrant>>;
	async fn local_origin(&mut self, task: Uuid) -> Result<Option<TaskOrigin>>;
	async fn remote_origin(&mut self, task: Uuid) -> Result<Option<TaskOrigin>>;
	async fn workspace(&mut self, id: Uuid) -> Result<Resource>;
	fn set_context(&mut self, context: Value);
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
}

pub mod admission;

pub mod visibility;

pub mod lease;
pub mod workspaces;

pub mod projection;

pub mod journals;

pub mod stream;

pub mod records;

pub mod state;

pub mod worker_tasks;

pub mod tools;

pub mod guard;

pub mod runs;

pub mod run_details;

pub mod inference;

pub mod resume;

pub mod worker_entry;

pub mod commands;

pub mod peer;
