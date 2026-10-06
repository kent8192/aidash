//! Outbound execution commits intent before I/O and never retries an uncertain external effect.
use crate::{
	Error, Result,
	ports::capabilities::outbound::{OutboundRepository, OutboundTransport},
};
use aidash_domain::capabilities::{
	outbound::{attempted, completed},
	records::Record,
};
use chrono::{Duration, Utc};
use std::time::Duration as StdDuration;
use uuid::Uuid;
async fn withdrawn(repository: &dyn OutboundRepository, record: &Record) -> Result<()> {
	loop {
		let mut scope = repository.begin(record).await?;
		let checked = async {
			let current = scope.load(record.id).await?;
			scope.authorize(&current).await?;
			Ok(None)
		}
		.await;
		scope.finish(checked).await?;
		tokio::time::sleep(StdDuration::from_millis(200)).await;
	}
}
pub async fn drive(
	repository: &dyn OutboundRepository,
	transport: &dyn OutboundTransport,
	id: Uuid,
) -> Result<()> {
	let snapshot = repository.snapshot(id).await?;
	if snapshot.state == "attempted" {
		return repository.fail(id, "OUTBOUND_EFFECT_UNCERTAIN").await;
	}
	let mut scope = repository.begin(&snapshot).await?;
	let accepted = async {
		let mut record = scope.load(id).await?;
		if record.state != "approved" {
			return Ok(None);
		}
		attempted(&mut record, Utc::now(), Utc::now() + Duration::seconds(90));
		scope.authorize(&record).await?;
		scope.update(&mut record).await?;
		Ok(Some(record))
	}
	.await;
	let Some(record) = scope.finish(accepted).await? else {
		return Ok(());
	};
	let policy = repository.fetch_policy();
	let fetch = async {
		let url = record.data["url"].as_str().ok_or(Error::Forbidden)?;
		transport.fetch(url, &record.data["targets"], &policy).await
	};
	let outcome = tokio::select! {
		result=tokio::time::timeout(StdDuration::from_secs(60),fetch)=>result.map_err(|_|Error::External("outbound timeout".into()))?,
		_=withdrawn(repository,&record)=>Err(Error::Forbidden),
	};
	let (status, bytes, final_url) = match outcome {
		Ok(result) => result,
		Err(Error::Invalid(code)) if code == "OUTBOUND_RESPONSE_LIMIT" => {
			return repository.fail(id, &code).await;
		}
		Err(_) => return repository.fail(id, "OUTBOUND_STOPPED").await,
	};
	let mut scope = repository.begin(&record).await?;
	let result = async {
		let mut record = scope.load(id).await?;
		let run = scope.authorize(&record).await?;
		let (file_id, digest) = scope.put(record.area_id, &bytes).await?;
		completed(&mut record, file_id, digest, status, &bytes, &final_url);
		scope.update(&mut record).await?;
		scope.event(&run, id, status).await?;
		Ok(None)
	}
	.await;
	scope.finish(result).await.map(|_| ())
}
pub async fn reconcile(
	repository: &dyn OutboundRepository,
	transport: &dyn OutboundTransport,
	id: Uuid,
) {
	if let Err(error) = Box::pin(drive(repository, transport, id)).await {
		tracing::warn!(%id,%error,"outbound operation stopped");
		let _ = repository.fail(id, "AUTHORITY_WITHDRAWN").await;
	}
}
#[cfg(test)]
mod tests;
