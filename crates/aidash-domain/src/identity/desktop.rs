//! Desktop session deadlines and refresh-family recovery rules.
use chrono::{DateTime, Duration, Utc};

pub const ACCESS_PREFIX: &str = "aidash_desktop_";
pub const REFRESH_PREFIX: &str = "aidash_refresh_";

pub fn valid_secret(value: &str) -> bool {
	(43..=128).contains(&value.len())
		&& value
			.bytes()
			.all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
}

pub struct Session {
	pub desktop: bool,
	pub revoked_at: Option<DateTime<Utc>>,
	pub expires_at: DateTime<Utc>,
	pub last_activity_at: DateTime<Utc>,
	pub idle_seconds: Option<i64>,
	pub access_expires_at: Option<DateTime<Utc>>,
}

impl Session {
	pub fn current(&self, now: DateTime<Utc>, require_access: bool) -> bool {
		self.desktop
			&& self.revoked_at.is_none()
			&& self.expires_at > now
			&& self
				.idle_seconds
				.is_some_and(|idle| self.last_activity_at > now - Duration::seconds(idle))
			&& (!require_access || self.access_expires_at.is_some_and(|expires| expires > now))
	}
}

#[derive(Debug, PartialEq, Eq)]
pub enum Rotation {
	Issue,
	Recover,
	Revoke,
}

/// A lost response may be recovered only with the same two prepared secrets.
pub fn rotation(
	previous_successor: Option<&[u8]>,
	proposed_successor: &[u8],
	successor_current: bool,
) -> Rotation {
	match previous_successor {
		None => Rotation::Issue,
		Some(previous) if previous == proposed_successor && successor_current => Rotation::Recover,
		Some(_) => Rotation::Revoke,
	}
}

#[cfg(test)]
mod tests;
