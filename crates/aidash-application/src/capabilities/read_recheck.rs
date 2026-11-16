//! Between worker boundaries, a running command or Live preview keeps being
//! checked against what its Run has read (ADR 0116). This runs apart from the
//! operation sweep, so a slow check never delays another Tenant's commands.
use crate::{
	Result,
	ports::capabilities::read_recheck::{Outcome, ReadRecheckRepository, Target},
};
use std::{
	collections::{BTreeMap, BTreeSet},
	time::{Duration, Instant},
};
use uuid::Uuid;

/// How often each target is checked. The first check after a target appears, or
/// after a restart, runs at once.
pub const PERIOD: Duration = Duration::from_secs(30);
/// How long a refused target waits before its stop is tried again after the first
/// failure. Each failure in a row doubles the wait, up to one period.
pub const RETRY_FROM: Duration = Duration::from_secs(5);

/// When each target was last checked. Losing this only adds one check.
#[derive(Default)]
pub struct Schedule {
	checked: BTreeMap<Target, Instant>,
	/// Refused targets whose stop failed: when to try again, and how many failures
	/// in a row. Every try is a check, which records a denial.
	failed: BTreeMap<Target, (Instant, u32)>,
}

impl Schedule {
	fn due(&self, target: Target, now: Instant) -> bool {
		if let Some((retry_at, _)) = self.failed.get(&target) {
			return now >= *retry_at;
		}
		self.checked
			.get(&target)
			.is_none_or(|checked| now.saturating_duration_since(*checked) >= PERIOD)
	}
	/// Schedule the next try after `failures` failed stops in a row.
	fn stop_failed(&mut self, target: Target, now: Instant, failures: u32) {
		let wait = RETRY_FROM
			.saturating_mul(1 << (failures - 1).min(5))
			.min(PERIOD);
		self.failed.insert(target, (now + wait, failures));
	}
	fn forget_ended(&mut self, targets: &[Target]) {
		let live: BTreeSet<Target> = targets.iter().copied().collect();
		self.checked.retain(|target, _| live.contains(target));
		self.failed.retain(|target, _| live.contains(target));
	}
}

pub async fn pass(
	repository: &dyn ReadRecheckRepository,
	schedule: &mut Schedule,
	now: Instant,
) -> Result<()> {
	let targets = repository.targets().await?;
	schedule.forget_ended(&targets);
	for target in targets {
		if !schedule.due(target, now) {
			continue;
		}
		schedule.checked.insert(target, now);
		// This try replaces the scheduled retry: if the check itself fails, the next
		// one waits a period.
		let failures = schedule.failed.remove(&target).map_or(0, |(_, n)| n);
		if let Err(error) = check(repository, schedule, target, now, failures).await {
			// Nothing was decided: try again at the next period. A refused target
			// whose stop failed is tried again sooner, because it should not run.
			tracing::warn!(?target, %error, "read recheck pending");
		}
	}
	Ok(())
}

async fn check(
	repository: &dyn ReadRecheckRepository,
	schedule: &mut Schedule,
	target: Target,
	now: Instant,
	failures: u32,
) -> Result<()> {
	match repository.check(target).await? {
		Outcome::Allowed => {
			// A confirmed check lets a withheld preview serve again.
			if let Target::LivePreview(preview) = target {
				repository.serve_again(preview).await?;
			}
			Ok(())
		}
		Outcome::Refused => {
			let stopped = repository.stop_refused(target).await;
			if stopped.is_err() {
				schedule.stop_failed(target, now, failures + 1);
			}
			stopped
		}
		Outcome::Unconfirmed => match target {
			Target::Operation(_) => {
				tracing::warn!(?target, "running command continues with unconfirmed reads");
				Ok(())
			}
			// The preview withholds its responses in an Outage Wait, and stops
			// as `node_unreachable` once that wait reaches its bound (ADR 0129).
			// Its own absolute limit may stop it first.
			Target::LivePreview(preview) => {
				let Some(since) = repository.withhold(preview).await? else {
					return Ok(());
				};
				tracing::warn!(?target, %since, "live preview withheld: reads unconfirmed");
				if chrono::Utc::now() - since < aidash_domain::run_state::OUTAGE_BOUND {
					return Ok(());
				}
				repository.stop_unconfirmed(preview).await
			}
		},
	}
}

#[cfg(test)]
mod tests;
