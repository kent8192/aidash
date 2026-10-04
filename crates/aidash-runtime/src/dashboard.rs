//! Provider refresh remains bounded and retries a failed pass after its interval.
use aidash_application::{Result, authorization::dashboard::DashboardAuthority};
use futures_util::{StreamExt, stream};
use std::{sync::Arc, time::Duration};
use tokio::sync::watch;

pub async fn refresh_active(
	authority: Arc<DashboardAuthority>,
	mut stopping: watch::Receiver<bool>,
) -> Result<()> {
	if !authority.configured() {
		return Ok(());
	}
	loop {
		match authority.active().await {
			Ok(accounts) => {
				stream::iter(
					accounts
						.into_iter()
						.map(|account| authority.refresh(account)),
				)
				.buffer_unordered(8)
				.for_each(|_| async {})
				.await;
			}
			Err(error) => {
				tracing::warn!(%error, "OIDC refresh pass failed; retrying after interval")
			}
		}
		tokio::select! {
			_ = tokio::time::sleep(Duration::from_secs(60)) => {},
			_ = stopping.changed() => if *stopping.borrow() { return Ok(()); },
		}
	}
}
