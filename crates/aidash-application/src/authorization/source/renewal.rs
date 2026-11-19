//! The Home renews a Remote Grant while the Run it started lives and the
//! delegation chain still holds (ADR 0109, amended 2026-11-17).
//!
//! A grant lasts one hour, so a remote Run that works long, or waits without
//! making calls for a person's answer, would be cut off at the hour. The Home
//! extends the grant by another hour once it is within [`RENEW_WITHIN`] of
//! expiry. It never revokes: a grant whose chain no longer holds is simply not
//! extended, and expires on its own.
//!
//! The chain is re-evaluated by the code that serves a description, which runs
//! the same source checks as preparation (`task.delegate`, `task.execute`,
//! `federation.execute` and each receiver dependency); this module adds no
//! policy of its own.
use crate::Result;
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

/// A grant is renewed once it has this long or less left.
pub const RENEW_WITHIN: Duration = Duration::minutes(20);
/// The lifetime a renewed grant gets, counted from the renewal (the longest
/// preparation allows).
pub const RENEWED_TTL_SECONDS: i64 = 3600;
/// Grants one pass of the Home's renewal loop looks at.
pub const RENEWAL_BATCH: u64 = 50;

/// What the Home knows about one grant, read on the database clock.
#[derive(Debug, Clone, PartialEq)]
pub struct GrantLife {
	pub revoked: bool,
	pub expires_at: DateTime<Utc>,
	/// The database clock when this was read.
	pub now: DateTime<Utc>,
	/// The remote Run this grant started is bound and not terminal.
	pub run_live: bool,
}

/// Where a grant stands, before the delegation chain is consulted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
	/// Revoked or already expired: a lapsed grant is never revived.
	Lapsed,
	/// The Run is not bound yet or has ended.
	NoLiveRun,
	/// More than [`RENEW_WITHIN`] is left.
	NotDue,
	/// Near expiry with a live Run: renew if the chain holds.
	Due,
}

impl GrantLife {
	pub fn standing(&self) -> Standing {
		let left = self.expires_at - self.now;
		if self.revoked || left <= Duration::zero() {
			Standing::Lapsed
		} else if !self.run_live {
			Standing::NoLiveRun
		} else if left > RENEW_WITHIN {
			Standing::NotDue
		} else {
			Standing::Due
		}
	}
}

/// What one renewal attempt did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Renewal {
	/// Extended by this call.
	Renewed,
	/// Not near expiry, so nothing to do.
	NotDue,
	/// Revoked, expired, unknown, or its Run has not started or has ended.
	Ineligible,
	/// The chain no longer holds; the grant is left to expire.
	ChainBroken,
	/// Another caller changed the expiry first, so it was already renewed.
	AlreadyRenewed,
}

#[async_trait]
pub trait RenewalRepository: Send + Sync {
	/// The grant's life now, or `None` when there is no such grant.
	async fn life(&self, id: Uuid) -> Result<Option<GrantLife>>;
	/// Whether the delegation chain still holds, evaluated as preparation does.
	/// A grant that cannot be evaluated now (an unreachable receiver) is `false`.
	async fn chain_holds(&self, id: Uuid) -> Result<bool>;
	/// Sets the expiry to `ttl_seconds` from now, only while the grant is still
	/// live and still expires at `seen`, so concurrent callers renew it once.
	/// Returns whether this call changed it.
	async fn extend(&self, id: Uuid, seen: DateTime<Utc>, ttl_seconds: i64) -> Result<bool>;
}

/// Renew `id` if it is due and its chain holds.
pub async fn renew<R: RenewalRepository>(repository: &R, id: Uuid) -> Result<Renewal> {
	let Some(life) = repository.life(id).await? else {
		return Ok(Renewal::Ineligible);
	};
	match life.standing() {
		Standing::Lapsed | Standing::NoLiveRun => return Ok(Renewal::Ineligible),
		Standing::NotDue => return Ok(Renewal::NotDue),
		Standing::Due => {}
	}
	if !repository.chain_holds(id).await? {
		return Ok(Renewal::ChainBroken);
	}
	extend(repository, id, &life).await
}

/// Renew `id` after the Home has just served a description or verification of
/// it, which re-evaluated the chain, so the chain is not evaluated again.
pub async fn renew_after_use<R: RenewalRepository>(repository: &R, id: Uuid) -> Result<Renewal> {
	let Some(life) = repository.life(id).await? else {
		return Ok(Renewal::Ineligible);
	};
	match life.standing() {
		Standing::Lapsed | Standing::NoLiveRun => Ok(Renewal::Ineligible),
		Standing::NotDue => Ok(Renewal::NotDue),
		Standing::Due => extend(repository, id, &life).await,
	}
}

async fn extend<R: RenewalRepository>(
	repository: &R,
	id: Uuid,
	life: &GrantLife,
) -> Result<Renewal> {
	Ok(
		if repository
			.extend(id, life.expires_at, RENEWED_TTL_SECONDS)
			.await?
		{
			Renewal::Renewed
		} else {
			Renewal::AlreadyRenewed
		},
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::Mutex;

	struct Fake {
		now: DateTime<Utc>,
		state: Mutex<GrantLife>,
		chain: bool,
		chain_checks: Mutex<u32>,
	}

	impl Fake {
		fn new(left_minutes: i64, chain: bool) -> Self {
			let now = Utc::now();
			Self {
				now,
				state: Mutex::new(GrantLife {
					revoked: false,
					expires_at: now + Duration::minutes(left_minutes),
					now,
					run_live: true,
				}),
				chain,
				chain_checks: Mutex::new(0),
			}
		}
		fn change(&self, edit: impl FnOnce(&mut GrantLife)) {
			edit(&mut self.state.lock().unwrap());
		}
		fn expires_at(&self) -> DateTime<Utc> {
			self.state.lock().unwrap().expires_at
		}
	}

	#[async_trait]
	impl RenewalRepository for Fake {
		async fn life(&self, _: Uuid) -> Result<Option<GrantLife>> {
			Ok(Some(self.state.lock().unwrap().clone()))
		}
		async fn chain_holds(&self, _: Uuid) -> Result<bool> {
			*self.chain_checks.lock().unwrap() += 1;
			Ok(self.chain)
		}
		async fn extend(&self, _: Uuid, seen: DateTime<Utc>, ttl: i64) -> Result<bool> {
			let mut state = self.state.lock().unwrap();
			if state.revoked || state.expires_at != seen || state.expires_at <= self.now {
				return Ok(false);
			}
			state.expires_at = self.now + Duration::seconds(ttl);
			Ok(true)
		}
	}

	#[tokio::test]
	async fn a_grant_near_expiry_with_a_holding_chain_gets_a_fresh_hour() {
		let fake = Fake::new(10, true);
		assert_eq!(renew(&fake, Uuid::nil()).await.unwrap(), Renewal::Renewed);
		assert_eq!(fake.expires_at(), fake.now + Duration::hours(1));
	}

	#[tokio::test]
	async fn a_grant_with_time_left_is_not_touched_and_its_chain_is_not_evaluated() {
		let fake = Fake::new(45, true);
		let before = fake.expires_at();
		assert_eq!(renew(&fake, Uuid::nil()).await.unwrap(), Renewal::NotDue);
		assert_eq!(fake.expires_at(), before);
		assert_eq!(*fake.chain_checks.lock().unwrap(), 0);
	}

	#[tokio::test]
	async fn a_broken_chain_is_left_to_expire_without_revocation() {
		let fake = Fake::new(5, false);
		let before = fake.expires_at();
		assert_eq!(
			renew(&fake, Uuid::nil()).await.unwrap(),
			Renewal::ChainBroken
		);
		assert_eq!(fake.expires_at(), before);
		assert!(!fake.state.lock().unwrap().revoked);
	}

	#[tokio::test]
	async fn a_revoked_expired_or_runless_grant_is_never_renewed() {
		for edit in [
			(|life: &mut GrantLife| life.revoked = true) as fn(&mut GrantLife),
			|life| life.expires_at = life.now - Duration::minutes(1),
			|life| life.run_live = false,
		] {
			let fake = Fake::new(5, true);
			fake.change(edit);
			let before = fake.expires_at();
			assert_eq!(
				renew(&fake, Uuid::nil()).await.unwrap(),
				Renewal::Ineligible
			);
			assert_eq!(fake.expires_at(), before);
		}
	}

	#[tokio::test]
	async fn a_second_renewal_in_the_same_window_changes_nothing() {
		let fake = Fake::new(5, true);
		assert_eq!(renew(&fake, Uuid::nil()).await.unwrap(), Renewal::Renewed);
		// The second caller read the life before the first renewed it.
		let stale = GrantLife {
			expires_at: fake.now + Duration::minutes(5),
			..fake.state.lock().unwrap().clone()
		};
		assert!(
			!fake
				.extend(Uuid::nil(), stale.expires_at, RENEWED_TTL_SECONDS)
				.await
				.unwrap()
		);
		assert_eq!(renew(&fake, Uuid::nil()).await.unwrap(), Renewal::NotDue);
		assert_eq!(fake.expires_at(), fake.now + Duration::hours(1));
	}

	#[tokio::test]
	async fn use_renews_without_evaluating_the_chain_again() {
		let fake = Fake::new(19, false);
		assert_eq!(
			renew_after_use(&fake, Uuid::nil()).await.unwrap(),
			Renewal::Renewed
		);
		assert_eq!(*fake.chain_checks.lock().unwrap(), 0);
	}

	#[test]
	fn the_window_is_twenty_minutes_inclusive() {
		let now = Utc::now();
		let life = |left: Duration| GrantLife {
			revoked: false,
			expires_at: now + left,
			now,
			run_live: true,
		};
		assert_eq!(life(Duration::minutes(20)).standing(), Standing::Due);
		assert_eq!(
			life(Duration::minutes(20) + Duration::seconds(1)).standing(),
			Standing::NotDue
		);
	}
}
