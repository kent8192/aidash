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
/// A Live preview can serve for up to 24 hours, so it does not outlast this many
/// unanswered checks in a row. A command ends at its own limit instead.
pub const UNCONFIRMED_LIMIT: u32 = 5;

/// When each target was last checked. Losing this only adds one check.
#[derive(Default)]
pub struct Schedule {
	checked: BTreeMap<Target, Instant>,
	unconfirmed: BTreeMap<Uuid, u32>,
}

impl Schedule {
	fn due(&self, target: Target, now: Instant) -> bool {
		self.checked
			.get(&target)
			.is_none_or(|checked| now.saturating_duration_since(*checked) >= PERIOD)
	}
	fn forget_ended(&mut self, targets: &[Target]) {
		let live: BTreeSet<Target> = targets.iter().copied().collect();
		self.checked.retain(|target, _| live.contains(target));
		self.unconfirmed
			.retain(|preview, _| live.contains(&Target::LivePreview(*preview)));
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
		if let Err(error) = check(repository, schedule, target).await {
			// Nothing was decided: try again at the next period. A refused target
			// whose stop failed is checked again sooner, because it should not run.
			tracing::warn!(?target, %error, "read recheck pending");
		}
	}
	Ok(())
}

async fn check(
	repository: &dyn ReadRecheckRepository,
	schedule: &mut Schedule,
	target: Target,
) -> Result<()> {
	match repository.check(target).await? {
		Outcome::Allowed => {
			if let Target::LivePreview(preview) = target {
				schedule.unconfirmed.remove(&preview);
			}
			Ok(())
		}
		Outcome::Refused => {
			let stopped = repository.stop_refused(target).await;
			if stopped.is_err() {
				schedule.checked.remove(&target);
			}
			stopped
		}
		Outcome::Unconfirmed => match target {
			Target::Operation(_) => {
				tracing::warn!(?target, "running command continues with unconfirmed reads");
				Ok(())
			}
			Target::LivePreview(preview) => {
				let misses = schedule.unconfirmed.entry(preview).or_default();
				*misses += 1;
				tracing::warn!(?target, misses = *misses, "live preview reads unconfirmed");
				if *misses < UNCONFIRMED_LIMIT {
					return Ok(());
				}
				repository.stop_unconfirmed(preview).await?;
				schedule.unconfirmed.remove(&preview);
				Ok(())
			}
		},
	}
}

#[cfg(test)]
mod tests;
