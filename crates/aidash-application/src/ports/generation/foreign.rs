//! Borrowed foreign-generation guards retain the caller's current authority transaction.
use crate::Result;
use aidash_domain::{
	Task,
	federation::execution::Description,
	generation::{
		intent::guards::{Authority, Record},
		remote::Ancestor,
		requests::Request,
	},
	policy::Resource,
	registry::EntityRef,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

#[async_trait]
pub trait ForeignGenerationGuard: Send {
	fn authority(&self) -> Authority;
	fn now(&self) -> DateTime<Utc>;
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	async fn job(&mut self, agent: &EntityRef) -> Result<Option<Request>>;
	/// Hold the same shared intent-row lock until the caller completes its transaction.
	async fn intent(&mut self, id: Uuid) -> Result<Option<Record>>;
	async fn lineage(&mut self, node: &str) -> Result<Vec<Ancestor>>;
	async fn workspace(&mut self, id: Uuid) -> Result<Resource>;
	async fn task_resource(&mut self, task: &Task) -> Result<Resource>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	/// Preserve exact Home/task/grant/admission, ACTIVE/quota and database-time predicates.
	async fn active(&mut self, description: &Description, admission: Uuid) -> Result<bool>;
}

#[async_trait]
pub trait ForeignGenerationBinding: Send {
	fn now(&self) -> DateTime<Utc>;
	/// Read the nonterminal Home/task binding under its original update lock.
	async fn prepared(&mut self, home: &str, task: Uuid) -> Result<Request>;
	async fn bind(&mut self, job: Uuid, grant: Uuid, admission: Uuid) -> Result<()>;
	async fn activate(&mut self, job: &Request) -> Result<()>;
}

pub mod home;

pub mod receiver;

pub mod maintenance;
