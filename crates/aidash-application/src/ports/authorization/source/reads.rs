//! Source output requires current reader and original producer authority in one transaction.
use crate::{
	Result,
	authorization::visits::ReadVisit,
	ports::authorization::source::{SourceAuthorityScope, SourcePeerScope},
};
use aidash_domain::{
	Task,
	federation::{
		dependencies::Reference,
		execution::home::{Grant, HomeBinding},
	},
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait ProducerScope: SourceAuthorityScope {
	async fn producer_semantic_sources(&mut self, grant: Uuid) -> Result<()>;
}
#[async_trait]
pub trait SourceReadScope: SourcePeerScope {
	fn output_visit(&self, id: Uuid) -> Option<ReadVisit>;
	fn protocol_version(&self) -> &str;
	async fn reader_grant(&mut self, node: &str, id: Uuid) -> Result<Option<Grant>>;
	async fn read_binding(&mut self, id: Uuid) -> Result<Option<HomeBinding>>;
	async fn read_task(&mut self, id: Uuid) -> Result<Task>;
	async fn grant_reads(&mut self, id: Uuid) -> Result<bool>;
	/// Restore viewer snapshot, identity, subjects, context and caches on drop, including cancellation.
	async fn producer<'a>(&'a mut self, grant: &Grant) -> Result<Box<dyn ProducerScope + 'a>>;
	fn record_admission(&mut self, node: &str, grant: Uuid, admission: Uuid) -> Result<()>;
	async fn output_record(&mut self, id: Uuid) -> Result<Option<(String, Value)>>;
	fn collecting_dependencies(&self) -> bool;
	/// The native scope owns collection cleanup until take_dependencies or drop.
	fn start_dependencies(&mut self);
	fn take_dependencies(&mut self) -> Vec<Reference>;
	async fn verify_dependencies(&mut self, pending: Vec<Reference>) -> Result<bool>;
}
