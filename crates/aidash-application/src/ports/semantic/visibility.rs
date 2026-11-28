//! Source disclosure borrows the same authority and row locks as its caller.
use crate::Result;
use aidash_domain::{Message, policy::Resource};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait SemanticDisclosureScope: Send {
	async fn unit(&mut self, _id: Uuid, _thread: Uuid) -> Result<Option<String>> {
		Err(crate::Error::Forbidden)
	}
	fn scoped(&self) -> bool;
	async fn deployment_administrator_visible(&mut self, thread: Uuid) -> Result<bool>;
	async fn thread_resource(&mut self, thread: Uuid) -> Result<Resource>;
	fn resource(&mut self, kind: &str, id: &str, attributes: Value) -> Resource;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;

	async fn message(&mut self, id: Uuid, thread: Uuid) -> Result<Option<Message>>;

	async fn message_visible(&mut self, message: &Message) -> Result<bool>;
}
