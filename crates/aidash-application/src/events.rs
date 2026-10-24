//! Durable events are retained on publication failure and deduplicated before wakeup.
use crate::{Result, ports::events::*};
use futures_util::{StreamExt, stream};

pub async fn publish(outbox: &dyn EventOutbox, broker: &dyn EventPublisher) -> Result<usize> {
	let batch = outbox.claim().await?;
	let mut attempts = stream::iter(batch.events().into_iter().map(|event| {
		let batch = batch.as_ref();
		async move {
			match broker.publish(&event).await {
				Ok(()) => {
					Ok::<usize, crate::Error>(usize::from(batch.finish(event.id, None).await?))
				}
				Err(error) => {
					batch.finish(event.id, Some(error.to_string())).await?;
					tracing::warn!(event_id=%event.id, %error, "outbox event retained for retry");
					Ok(0)
				}
			}
		}
	}))
	.buffer_unordered(8);
	let mut published = 0;
	while let Some(result) = attempts.next().await {
		published += result?;
	}
	Ok(published)
}

pub async fn receive(
	inbox: &dyn EventInbox,
	subscription: &mut dyn EventSubscription,
	wakeup: &dyn EventWakeup,
) -> Result<()> {
	let delivery = subscription.next().await?;
	let Some(id) = delivery.event_id() else {
		delivery.discard().await?;
		tracing::warn!("discarded malformed broker event");
		return Ok(());
	};
	if inbox.receive(id).await? {
		wakeup.wake();
	}
	// A crash after commit is recovered by the worker's durable scan. A replay
	// acknowledges the existing inbox entry without triggering duplicate work.
	delivery.acknowledge().await
}

#[cfg(test)]
mod tests;
