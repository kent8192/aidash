//! Read leases preserve one caller authority through paginated disclosure.
use super::visibility::GenerationVisibility;
use crate::Result;
use aidash_domain::{
	generation::{
		policy::Spec,
		requests::{History, Request, Usage},
	},
	identity::Principal,
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait GenerationPages: Send {
	async fn page(&mut self, tenant: &str, offset: usize) -> Result<Vec<Request>>;
}
#[async_trait]
pub trait GenerationReadScope: GenerationVisibility {
	async fn job(&mut self, tenant: &str, id: Uuid) -> Result<Option<Request>>;
	async fn history(&mut self, tenant: &str, id: Uuid) -> Result<Vec<History>>;
	async fn usage(&mut self, tenant: &str, id: Uuid) -> Result<Usage>;
	async fn specification(&mut self, tenant: &str, id: Uuid) -> Result<Value>;
	async fn finish_requests(self: Box<Self>, result: Result<Vec<Request>>)
	-> Result<Vec<Request>>;
	async fn finish_history(self: Box<Self>, result: Result<Vec<History>>) -> Result<Vec<History>>;
	async fn finish_usage(self: Box<Self>, result: Result<Usage>) -> Result<Usage>;
	async fn finish_specification(self: Box<Self>, result: Result<Spec>) -> Result<Spec>;
}
#[async_trait]
pub trait GenerationReads: Send + Sync {
	fn principal(&self) -> &Principal;
	async fn pages(&self) -> Result<Box<dyn GenerationPages>>;
	async fn begin_subject(&self) -> Result<Box<dyn GenerationReadScope>>;
	async fn operator_history(&self, tenant: &str, id: Uuid) -> Result<Vec<History>>;
	async fn operator_usage(&self, tenant: &str, id: Uuid) -> Result<Usage>;
	async fn operator_specification(&self, tenant: &str, id: Uuid) -> Result<Value>;
}
