//! The outbound worker bounds concurrency and drains the accepted batch before stopping.
use aidash_application::{
	Result,
	capabilities::outbound,
	ports::capabilities::outbound::{OutboundRepository, OutboundTransport},
};
use futures_util::{StreamExt, stream};
use std::time::Duration;
use tokio::sync::watch;
pub async fn run(
	repository: &dyn OutboundRepository,
	transport: &dyn OutboundTransport,
	mut stopping: watch::Receiver<bool>,
) -> Result<()> {
	loop {
		if *stopping.borrow() {
			return Ok(());
		}
		let ids = repository.active_operations().await?;
		stream::iter(ids)
			.for_each_concurrent(8, |id| async move {
				outbound::reconcile(repository, transport, id).await;
			})
			.await;
		tokio::select! {_=stopping.changed()=>{},_=tokio::time::sleep(Duration::from_millis(300))=>{}}
	}
}
#[cfg(test)]
mod tests;
