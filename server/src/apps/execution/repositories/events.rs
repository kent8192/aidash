//! Native event publication and inbox admission retain their original fences.
use crate::{
	apps::execution::models::{Event as Record, Inbox},
	store::Store,
};
use aidash_application::{Result, ports::events::*};
use aidash_domain::Event;
use async_trait::async_trait;
use uuid::Uuid;

pub(crate) struct Outbox {
	pub store: Store,
}
struct Batch {
	records: Vec<Record>,
	pool: crate::database::native::Pool,
	// TransactionExecutor is Send; the mutex only makes the owned RAII guard
	// shareable while independent publication acknowledgements run concurrently.
	_visibility: tokio::sync::Mutex<crate::transactions::gate::ReadLease>,
}
#[async_trait]
impl EventOutbox for Outbox {
	async fn claim(&self) -> Result<Box<dyn EventOutboxBatch>> {
		let visibility = crate::transactions::gate::ReadLease::begin(&self.store).await?;
		let records = Record::claim_outbox(&self.store.pool).await?;
		Ok(Box::new(Batch {
			records,
			pool: self.store.pool.clone(),
			_visibility: tokio::sync::Mutex::new(visibility),
		}))
	}
}
#[async_trait]
impl EventOutboxBatch for Batch {
	fn events(&self) -> Vec<Event> {
		self.records.iter().cloned().map(Event::from).collect()
	}
	async fn finish(&self, id: Uuid, error: Option<String>) -> Result<bool> {
		let record = self
			.records
			.iter()
			.find(|record| record.id == id)
			.ok_or_else(|| {
				aidash_application::Error::Conflict("event is not in the claimed batch".into())
			})?;
		record
			.finish_publication(&self.pool, error)
			.await
			.map_err(Into::into)
	}
}
pub(crate) struct NativeInbox {
	pub pool: crate::database::native::Pool,
}
#[async_trait]
impl EventInbox for NativeInbox {
	async fn receive(&self, id: Uuid) -> Result<bool> {
		Inbox::receive(&self.pool, id).await.map_err(Into::into)
	}
}
impl EventWakeup for crate::federation::Federation {
	fn wake(&self) {
		self.notify.notify_waiters();
	}
}
