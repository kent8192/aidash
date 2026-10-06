//! Opt-in deterministic fault cuts for real-process acceptance. The directory
//! and exact transaction/cut must both be set by the process owner. Normal
//! deployments perform no filesystem I/O and expose no fault-control API.
use crate::{Error, Result};
use uuid::Uuid;

pub(crate) async fn cut(id: Uuid, point: &str) -> Result<()> {
	let Ok(selected) = std::env::var("AIDASH_TRANSACTION_FAULT") else {
		return Ok(());
	};
	if selected != format!("{id}:{point}") {
		return Ok(());
	}
	let directory = std::env::var_os("AIDASH_TRANSACTION_FAULT_DIR")
		.ok_or_else(|| Error::Invalid("transaction fault directory is required".into()))?;
	let directory = std::path::PathBuf::from(directory);
	let marker = directory.join(format!("{id}.{point}.reached"));
	tokio::fs::write(&marker, b"reached\n")
		.await
		.map_err(|e| Error::External(format!("fault marker: {e}")))?;
	let release = directory.join(format!("{id}.{point}.release"));
	while !tokio::fs::try_exists(&release)
		.await
		.map_err(|e| Error::External(format!("fault release: {e}")))?
	{
		tokio::time::sleep(std::time::Duration::from_millis(20)).await;
	}
	Ok(())
}
