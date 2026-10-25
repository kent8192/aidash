//! Bounded background synchronization, owned by the process Supervisor.
use crate::{Result, federation::Federation};
use aidash_application::tenancy::TenancyScope;
use tokio::sync::watch;

pub async fn synchronize(runtime: &Federation) -> Result<()> {
	let mut failed = None;
	for connection in &runtime.config.marketplace.connections {
		let result = async {
			let client = runtime.config.marketplace.client_for(connection)?;
			let recalls = client.recalls().await?;
			runtime
				.store
				.pool
				.atomic(&TenancyScope::NoTenant, async |tx| {
					crate::apps::registry::repositories::marketplace_recalls::retain(tx, &recalls)
						.await
				})
				.await
		}
		.await;
		if let Err(error) = result {
			failed = Some(error);
		}
	}
	if let Some(error) = failed {
		Err(error)
	} else {
		Ok(())
	}
}
pub async fn run(runtime: Federation, mut stopping: watch::Receiver<bool>) -> Result<()> {
	loop {
		if *stopping.borrow() {
			return Ok(());
		}
		tokio::select! {
		 result=synchronize(&runtime)=>if result.is_err(){tracing::warn!("Marketplace Recall synchronization unavailable; retained stops remain active");},
		 _=stopping.changed()=>return Ok(()),
		}
		tokio::select! {_=stopping.changed()=>return Ok(()),_=tokio::time::sleep(std::time::Duration::from_secs(runtime.config.marketplace.poll_seconds))=>{}}
	}
}
