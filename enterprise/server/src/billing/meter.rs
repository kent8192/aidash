//! The meters: Execution Pod time from the runner's Pod start and stop, and a
//! daily sample of the bytes of files each Tenant stores. Both run on Cloud
//! and Dedicated. Metering of a Tenant stops while a Tenant Deletion is
//! recorded for it, and the records outlive the Tenant's purge until the
//! Deployment's retention period ends. Egress gateway bytes are not metered.
use super::settings::Settings;
use super::store;
use aidash_application::usage::{PodTime, StorageSample, UsageEvents};
use aidash_server::database::native::Pool;
use aidash_server::store::Store;
use async_trait::async_trait;
use chrono::{DateTime, Datelike, Months, NaiveDate, Utc};
use std::time::Duration;
use tokio::sync::watch;

pub struct Meter {
	pub pool: Pool,
}

#[async_trait]
impl UsageEvents for Meter {
	async fn pod_time(&self, event: PodTime) -> aidash_application::Result<()> {
		store::facts::pod_time(&self.pool, event)
			.await
			.map_err(Into::into)
	}
	async fn storage_sampled(&self, samples: Vec<StorageSample>) -> aidash_application::Result<()> {
		store::facts::storage_sampled(&self.pool, samples)
			.await
			.map_err(Into::into)
	}
}

/// When records older than `years` before `now` expire.
pub fn cutoff(now: DateTime<Utc>, years: u32) -> DateTime<Utc> {
	now.checked_sub_months(Months::new(years.saturating_mul(12)))
		.unwrap_or(DateTime::<Utc>::MIN_UTC)
}

/// The start of the UTC calendar month of `now`: Free usage counts from it.
pub fn month_start(now: DateTime<Utc>) -> DateTime<Utc> {
	NaiveDate::from_ymd_opt(now.year(), now.month(), 1)
		.expect("the first of a month")
		.and_hms_opt(0, 0, 0)
		.expect("midnight")
		.and_utc()
}

const TICK: Duration = Duration::from_secs(3600);

/// One pass of the hourly service: the day's storage sample, taken once per
/// UTC day, and the retention sweep.
pub async fn maintain(
	store: Store,
	settings: Settings,
	mut stopping: watch::Receiver<bool>,
) -> aidash_server::Result<()> {
	let mut sampled = None::<NaiveDate>;
	loop {
		if *stopping.borrow() {
			return Ok(());
		}
		let now = Utc::now();
		if sampled != Some(now.date_naive()) {
			match aidash_server::apps::execution::services::storage_usage::samples(&store, now)
				.await
			{
				Ok(samples) => match store.usage.storage_sampled(samples).await {
					Ok(()) => sampled = Some(now.date_naive()),
					Err(error) => tracing::warn!(%error, "recording the storage sample failed"),
				},
				Err(error) => tracing::warn!(%error, "sampling stored bytes failed"),
			}
		}
		if let Err(error) = store::sweep(
			&store.control_pool,
			cutoff(now, settings.usage_retention_years),
			now,
		)
		.await
		{
			tracing::warn!(%error, "the usage retention sweep failed");
		}
		tokio::select! {
			_ = stopping.changed() => {}
			_ = tokio::time::sleep(TICK) => {}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use chrono::{Duration, TimeZone};

	#[test]
	fn retention_counts_whole_years_back_and_months_start_on_the_first() {
		let now = Utc.with_ymd_and_hms(2026, 10, 19, 12, 30, 0).unwrap();
		assert_eq!(
			cutoff(now, 7),
			Utc.with_ymd_and_hms(2019, 10, 19, 12, 30, 0).unwrap()
		);
		assert_eq!(
			month_start(now),
			Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap()
		);
		assert!(cutoff(now, u32::MAX) < now - Duration::days(365 * 1000));
	}
}
