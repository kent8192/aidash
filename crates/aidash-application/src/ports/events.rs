//! Durable publication and notification ports independent of broker protocols.
use crate::Result;
use aidash_domain::Event;
use async_trait::async_trait;
use uuid::Uuid;

/// A claimed batch owns its visibility lease until it is dropped. Finishing a
/// publication must compare the original claim, so a late response cannot
/// overwrite a newer publisher's result.
#[async_trait]
pub trait EventOutboxBatch: Send + Sync {
	fn events(&self) -> Vec<Event>;
	async fn finish(&self, event: Uuid, error: Option<String>) -> Result<bool>;
}
#[async_trait]
pub trait EventOutbox: Send + Sync {
	async fn claim(&self) -> Result<Box<dyn EventOutboxBatch>>;
}
#[async_trait]
pub trait EventPublisher: Send + Sync {
	async fn publish(&self, event: &Event) -> Result<()>;
}
#[async_trait]
pub trait EventDelivery: Send + Sync {
	fn event_id(&self) -> Option<Uuid>;
	async fn acknowledge(self: Box<Self>) -> Result<()>;
	async fn discard(self: Box<Self>) -> Result<()>;
}
#[async_trait]
pub trait EventSubscription: Send {
	async fn next(&mut self) -> Result<Box<dyn EventDelivery>>;
}
#[async_trait]
pub trait EventInbox: Send + Sync {
	/// Return only after the durable deduplication record has committed.
	async fn receive(&self, event: Uuid) -> Result<bool>;
}
pub trait EventWakeup: Send + Sync {
	fn wake(&self);
}
