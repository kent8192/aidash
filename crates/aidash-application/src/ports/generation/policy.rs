//! A policy scope owns one transaction through its authorization and history.
use crate::Result;
use aidash_domain::{
	generation::policy::{Policy, Spec},
	identity::Principal,
	registry::EntityRef,
};
use async_trait::async_trait;
use serde_json::Value;

#[async_trait]
pub trait PolicySession: Send {
	async fn decide(&mut self, id: &str, action: &str) -> Result<bool>;
	async fn bundle(&mut self, tenant: &str) -> Result<Value>;
	async fn previous(&mut self, tenant: &str, id: &str, revision: i64) -> Result<Option<Value>>;
	async fn approved(&mut self, tenant: &str, reference: &EntityRef) -> Result<Option<Value>>;
	async fn compare_and_set(
		&mut self,
		tenant: &str,
		id: &str,
		expected: i64,
		spec: &Spec,
	) -> Result<Option<i64>>;
	async fn history(
		&mut self,
		tenant: &str,
		id: &str,
		revision: i64,
		spec: &Spec,
		actor: &str,
	) -> Result<()>;
	async fn load(&mut self, tenant: &str, id: &str, exclusive: bool) -> Result<Policy>;
	// Denied subject decisions retain the adapter's durable denial audit, while
	// every protected write rolls back on failure, cancellation or scope drop.
	async fn finish_update(self: Box<Self>, result: Result<Policy>) -> Result<Policy>;
	async fn finish_list(self: Box<Self>, result: Result<Vec<Policy>>) -> Result<Vec<Policy>>;
}
#[async_trait]
pub trait GenerationPolicies: Send + Sync {
	fn principal(&self) -> &Principal;
	async fn ids(&self, tenant: &str) -> Result<Vec<String>>;
	async fn begin(&self, tenant: &str, exclusive: bool) -> Result<Box<dyn PolicySession>>;
}
