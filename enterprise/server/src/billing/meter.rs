//! The meters: Execution Pod time from the runner's Pod start and stop, and a
//! daily sample of the bytes of files each Tenant stores. Both run on Cloud
//! and Dedicated. Metering of a Tenant stops while a Tenant Deletion is
//! recorded for it, and the records outlive the Tenant's purge until the
//! Deployment's retention period ends. Egress gateway bytes are not metered.
use super::settings::Settings;
use super::store::{self, Deletion};
use aidash_application::tenancy::TenancyScope;
use aidash_application::usage::{PodTime, StorageSample, UsageEvents};
use aidash_server::database::native::Pool;
use aidash_server::store::Store;
use async_trait::async_trait;
use chrono::{DateTime, Datelike, Months, NaiveDate, Utc};
use std::collections::BTreeSet;
use std::time::Duration;
use tokio::sync::watch;

pub struct Meter {
	pub pool: Pool,
}

/// How much of `[start, end]` lies outside every recorded Deletion.
pub fn metered_millis(start: DateTime<Utc>, end: DateTime<Utc>, deletions: &[Deletion]) -> i64 {
	let total = (end - start).num_milliseconds().max(0);
	let stopped: i64 = deletions
		.iter()
		.map(|(from, until)| {
			let from = (*from).max(start);
			let until = until.unwrap_or(end).min(end);
			(until - from).num_milliseconds().max(0)
		})
		.sum();
	(total - stopped).max(0)
}

/// Whether a Deletion was recorded and not restored at `at`.
pub fn deleted_at(at: DateTime<Utc>, deletions: &[Deletion]) -> bool {
	deletions
		.iter()
		.any(|(from, until)| *from <= at && until.is_none_or(|until| at < until))
}

#[async_trait]
impl UsageEvents for Meter {
	async fn pod_time(&self, event: PodTime) -> aidash_application::Result<()> {
		self.pool
			.atomic(&TenancyScope::NoTenant, async |tx| {
				let deletions = store::deletions(tx, std::slice::from_ref(&event.tenant)).await?;
				let spans = deletions.get(&event.tenant).map_or(&[][..], Vec::as_slice);
				let millis = metered_millis(event.started_at, event.ended_at, spans);
				if millis == 0 {
					return Ok(());
				}
				store::record_pod_usage(
					tx,
					event.allocation,
					&event.tenant,
					event.started_at,
					event.ended_at,
					millis,
				)
				.await
			})
			.await
			.map_err(Into::into)
	}

	async fn storage_sampled(&self, samples: Vec<StorageSample>) -> aidash_application::Result<()> {
		let tenants: Vec<String> = samples
			.iter()
			.map(|sample| sample.tenant.clone())
			.collect::<BTreeSet<_>>()
			.into_iter()
			.collect();
		self.pool
			.atomic(&TenancyScope::NoTenant, async |tx| {
				let deletions = store::deletions(tx, &tenants).await?;
				for sample in &samples {
					let spans = deletions.get(&sample.tenant).map_or(&[][..], Vec::as_slice);
					if deleted_at(sample.sampled_at, spans) {
						continue;
					}
					let bytes = i64::try_from(sample.bytes).unwrap_or(i64::MAX);
					store::record_storage_sample(tx, &sample.tenant, sample.sampled_at, bytes)
						.await?;
				}
				Ok(())
			})
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

	fn at(hour: u32) -> DateTime<Utc> {
		Utc.with_ymd_and_hms(2026, 10, 19, hour, 0, 0).unwrap()
	}

	#[test]
	fn a_pod_that_ran_before_a_deletion_is_metered_only_until_it() {
		let deletions = [(at(10), None)];
		assert_eq!(metered_millis(at(8), at(12), &deletions), 2 * 3_600_000);
		assert_eq!(metered_millis(at(11), at(12), &deletions), 0);
		assert_eq!(metered_millis(at(8), at(9), &deletions), 3_600_000);
	}

	#[test]
	fn a_restore_resumes_metering_and_the_grace_period_is_not_metered() {
		let deletions = [(at(10), Some(at(14)))];
		assert_eq!(metered_millis(at(9), at(16), &deletions), 3 * 3_600_000);
		assert!(deleted_at(at(10), &deletions));
		assert!(deleted_at(at(13), &deletions));
		assert!(!deleted_at(at(14), &deletions));
		assert!(!deleted_at(at(9), &deletions));
	}

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
