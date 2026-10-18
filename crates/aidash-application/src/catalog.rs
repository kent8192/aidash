//! One process-wide copy of the OpenRouter model catalog (#362). Every member
//! can read it, so the server fetches it at most once per period, whatever the
//! number of callers, and the egress to OpenRouter does not grow with them.
use crate::{Result, ports::ModelCatalog};
use aidash_domain::catalog::{CatalogModel, CatalogSnapshot};
use chrono::{DateTime, Duration, Utc};

/// How long a fetched catalog is served before it is refreshed.
pub const FRESH_FOR: Duration = Duration::minutes(10);
/// How long a catalog is served, marked stale, while refreshes fail.
pub const STALE_FOR: Duration = Duration::hours(24);
/// The least time between two attempts after a failed refresh.
pub const RETRY_AFTER: Duration = Duration::seconds(30);

struct Fetched {
	models: Vec<CatalogModel>,
	at: DateTime<Utc>,
}

#[derive(Default)]
struct State {
	fetched: Option<Fetched>,
	failed_at: Option<DateTime<Utc>>,
}

pub struct CatalogCache {
	state: tokio::sync::Mutex<State>,
}

impl CatalogCache {
	pub const fn new() -> Self {
		Self {
			state: tokio::sync::Mutex::const_new(State {
				fetched: None,
				failed_at: None,
			}),
		}
	}

	/// The catalog, refreshed from `source` when it is older than
	/// [`FRESH_FOR`]. The lock is held across the refresh, so concurrent
	/// callers wait for one fetch instead of starting their own.
	pub async fn get(
		&self,
		source: &dyn ModelCatalog,
		now: impl Fn() -> DateTime<Utc>,
	) -> Result<CatalogSnapshot> {
		let mut state = self.state.lock().await;
		let now = now();
		let snapshot = |fetched: &Fetched, stale: bool| CatalogSnapshot {
			models: fetched.models.clone(),
			stale,
			fetched_at: fetched.at,
		};
		if let Some(fetched) = &state.fetched
			&& now - fetched.at < FRESH_FOR
		{
			return Ok(snapshot(fetched, false));
		}
		let recently_failed = state
			.failed_at
			.is_some_and(|failed| now - failed < RETRY_AFTER);
		let refreshed = if recently_failed {
			None
		} else {
			Some(source.models().await)
		};
		match refreshed {
			Some(Ok(models)) => {
				state.failed_at = None;
				let fetched = state.fetched.insert(Fetched { models, at: now });
				Ok(snapshot(fetched, false))
			}
			failure => {
				if failure.is_some() {
					state.failed_at = Some(now);
				}
				match &state.fetched {
					Some(fetched) if now - fetched.at < STALE_FOR => Ok(snapshot(fetched, true)),
					_ => match failure {
						Some(Err(error)) => Err(error),
						_ => Err(crate::Error::External(
							"the OpenRouter model catalog is unavailable".into(),
						)),
					},
				}
			}
		}
	}
}

impl Default for CatalogCache {
	fn default() -> Self {
		Self::new()
	}
}

#[cfg(test)]
mod tests;
