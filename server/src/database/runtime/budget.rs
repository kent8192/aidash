//! PostgreSQL connection allocation shared by a server or worker process.
use crate::{Error, Result};

const BUDGET_ENV: &str = "AIDASH_DATABASE_CONNECTION_BUDGET";
const REPLICAS_ENV: &str = "AIDASH_DATABASE_CONNECTION_REPLICAS";
// Twelve connections leave three Control, seven Work and two Recovery slots.
const MINIMUM: u32 = 12;
// Local/Compose keep the former process maximum: main and control (16 each),
// restoration (4), scratch upload (5), and three worker/recovery pools (8 each).
const LOCAL_BUDGET: u32 = 16 + 16 + 4 + 5 + 3 * 8;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Budget {
	pub control: u32,
	pub work: u32,
	pub recovery: u32,
	pub replicas: u32,
}

fn env_u32(name: &str, default: u32) -> Result<u32> {
	match std::env::var(name) {
		Ok(value) => value
			.parse()
			.map_err(|_| Error::Invalid(format!("{name} must be a whole number"))),
		Err(std::env::VarError::NotPresent) => Ok(default),
		Err(error) => Err(Error::Invalid(format!("invalid {name}: {error}"))),
	}
}

impl Budget {
	pub(super) fn from_env() -> Result<Self> {
		Self::split(
			env_u32(BUDGET_ENV, LOCAL_BUDGET)?,
			env_u32(REPLICAS_ENV, 1)?,
		)
	}

	fn split(total: u32, replicas: u32) -> Result<Self> {
		if total < MINIMUM || replicas == 0 {
			return Err(Error::Invalid(format!(
				"{BUDGET_ENV} must be at least {MINIMUM} per process and {REPLICAS_ENV} must be positive",
			)));
		}
		// A quarter isolates state transitions and advisory locks from data work.
		let control = total / 4;
		// Three fifths serve Tenant data, including requests, uploads and workers.
		let work = (u64::from(total) * 60 / 100) as u32;
		// The remaining 15%, including integer rounding, serves background recovery.
		let recovery = total - control - work;
		Ok(Self {
			control,
			work,
			recovery,
			replicas,
		})
	}

	pub(super) fn planned_connections(&self) -> u64 {
		u64::from(self.control + self.work + self.recovery) * u64::from(self.replicas)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn pool_split_spends_the_budget_once_per_replica() {
		assert_eq!(
			Budget::split(65, 4).unwrap(),
			Budget {
				control: 16,
				work: 39,
				recovery: 10,
				replicas: 4
			},
		);
		let minimum = Budget::split(12, 3).unwrap();
		assert_eq!((minimum.control, minimum.work, minimum.recovery), (3, 7, 2));
		assert_eq!(minimum.planned_connections(), 36);
		assert!(Budget::split(11, 3).is_err());
	}
}
