//! Normal Deployment Key rollover and receiver-local trust (ADR 0084).
//! Cryptographic proof verification belongs to the capability adapter; these
//! rules apply only after the complete incoming request has authenticated.
use super::DeploymentKeyId;
use crate::{Error, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicDeploymentKey {
	pub key_id: DeploymentKeyId,
	pub public_key: [u8; 32],
}

impl PublicDeploymentKey {
	pub fn new(key_id: DeploymentKeyId, public_key: [u8; 32]) -> Result<Self> {
		let key = Self { key_id, public_key };
		key.validate()?;
		Ok(key)
	}

	pub fn validate(&self) -> Result<()> {
		if self.key_id != DeploymentKeyId::of_public_key(&self.public_key) {
			return Err(invalid("a Deployment Key ID must digest its public key"));
		}
		Ok(())
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RolloverSchedule {
	pub published_at: DateTime<Utc>,
	pub activates_at: DateTime<Utc>,
	pub retires_at: DateTime<Utc>,
}

impl RolloverSchedule {
	pub fn new(published_at: DateTime<Utc>) -> Result<Self> {
		let activates_at = published_at
			.checked_add_signed(Duration::hours(24))
			.ok_or_else(|| invalid("Deployment Key activation overflows"))?;
		let retires_at = activates_at
			.checked_add_signed(Duration::days(7))
			.ok_or_else(|| invalid("Deployment Key retirement overflows"))?;
		Ok(Self {
			published_at,
			activates_at,
			retires_at,
		})
	}

	pub fn validate(&self) -> Result<()> {
		if *self != Self::new(self.published_at)? {
			return Err(invalid(
				"a normal rollover activates after 24 hours and retires after seven days",
			));
		}
		Ok(())
	}

	pub fn in_overlap(&self, now: DateTime<Utc>) -> bool {
		self.activates_at <= now && now < self.retires_at
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetiringDeploymentKey {
	pub key: PublicDeploymentKey,
	pub schedule: RolloverSchedule,
}

/// Keep the last retirement after its verification window closes: it prevents
/// backdated transitions and a return to the immediately preceding key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedDeploymentKeys {
	pub current: PublicDeploymentKey,
	pub retiring: Option<RetiringDeploymentKey>,
	pub quarantined: bool,
}

impl TrustedDeploymentKeys {
	pub fn new(current: PublicDeploymentKey) -> Result<Self> {
		current.validate()?;
		Ok(Self {
			current,
			retiring: None,
			quarantined: false,
		})
	}

	/// Only the current key may introduce one successor during its overlap.
	/// The caller must verify the predecessor proof AND successor outer token
	/// (including request binding) before persisting this state change.
	pub fn accept_successor(
		&mut self,
		predecessor: &DeploymentKeyId,
		successor: PublicDeploymentKey,
		schedule: RolloverSchedule,
		now: DateTime<Utc>,
	) -> Result<()> {
		self.current.validate()?;
		successor.validate()?;
		schedule.validate()?;
		if self.quarantined
			|| predecessor != &self.current.key_id
			|| successor.key_id == self.current.key_id
			|| !schedule.in_overlap(now)
		{
			return Err(invalid(
				"Deployment Key transition is not currently trusted",
			));
		}
		if let Some(retiring) = &self.retiring {
			retiring.schedule.validate()?;
			if schedule.published_at < retiring.schedule.retires_at
				|| successor.key_id == retiring.key.key_id
			{
				return Err(invalid(
					"Deployment Key transitions cannot overlap or move backward",
				));
			}
		}
		let predecessor = std::mem::replace(&mut self.current, successor);
		self.retiring = Some(RetiringDeploymentKey {
			key: predecessor,
			schedule,
		});
		Ok(())
	}

	pub fn trusted_key(
		&self,
		kid: &DeploymentKeyId,
		now: DateTime<Utc>,
	) -> Option<&PublicDeploymentKey> {
		if self.quarantined {
			return None;
		}
		if kid == &self.current.key_id {
			return Some(&self.current);
		}
		self.retiring
			.as_ref()
			.filter(|retiring| kid == &retiring.key.key_id && retiring.schedule.in_overlap(now))
			.map(|retiring| &retiring.key)
	}

	pub fn quarantine(&mut self) {
		self.quarantined = true;
	}

	/// The existing record administrator independently confirms the replacement.
	/// This resets authentication trust only, never the record's authorization.
	pub fn manual_repin(&mut self, key: PublicDeploymentKey) -> Result<()> {
		*self = Self::new(key)?;
		Ok(())
	}
}

fn invalid(message: &str) -> Error {
	Error::Invalid(message.into())
}
