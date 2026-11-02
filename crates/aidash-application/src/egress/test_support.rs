//! An in-memory scope authority for gateway tests, never for deployment.
use super::{ConnectionRecord, Direction, EgressScopes, ScopeSettings};
use crate::{Error, Result};
use aidash_domain::egress::ScopeId;
use async_trait::async_trait;
use chrono::NaiveDate;
use std::{
	collections::HashMap,
	sync::Mutex,
	time::{Duration, Instant},
};

struct State {
	suspended: bool,
	window: Instant,
	connections: u32,
	notifications: u32,
	usage: HashMap<NaiveDate, (u64, u64)>,
	records: Vec<ConnectionRecord>,
}

/// A single configured credential and scope; updates are atomic within a process.
pub struct MemoryScopes {
	credential: String,
	settings: ScopeSettings,
	state: Mutex<State>,
}
impl MemoryScopes {
	pub fn new(credential: String, settings: ScopeSettings) -> Self {
		Self {
			credential,
			settings,
			state: Mutex::new(State {
				suspended: false,
				window: Instant::now(),
				connections: 0,
				notifications: 0,
				usage: HashMap::new(),
				records: Vec::new(),
			}),
		}
	}
	pub fn records(&self) -> Vec<ConnectionRecord> {
		self.state.lock().unwrap().records.clone()
	}
	pub fn notifications(&self) -> u32 {
		self.state.lock().unwrap().notifications
	}
	fn check(&self, id: &ScopeId) -> Result<()> {
		if id != &self.settings.id {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
}
#[async_trait]
impl EgressScopes for MemoryScopes {
	async fn resolve(&self, credential: &str) -> Result<ScopeSettings> {
		if credential != self.credential {
			return Err(Error::Unauthorized);
		}
		Ok(self.settings.clone())
	}
	async fn admit_external(
		&self,
		_: &str,
		_: &str,
		_: &str,
		_: &str,
		_: &str,
	) -> Result<super::Admission> {
		// This test scope has no External services configured.
		Err(Error::Forbidden)
	}
	async fn admit_connection(&self, scope: &ScopeSettings) -> Result<()> {
		self.check(&scope.id)?;
		let mut s = self.state.lock().unwrap();
		if s.suspended {
			return Err(Error::Refused("egress_suspended".into()));
		}
		if s.window.elapsed() >= Duration::from_secs(60) {
			s.window = Instant::now();
			s.connections = 0;
		}
		s.connections = s.connections.saturating_add(1);
		if s.connections > self.settings.anomaly.connections_per_minute {
			s.suspended = true;
			s.notifications += 1;
			return Err(Error::Refused("egress_suspended".into()));
		}
		Ok(())
	}
	async fn check_active(&self, scope: &ScopeSettings) -> Result<()> {
		self.check(&scope.id)?;
		if self.state.lock().unwrap().suspended {
			return Err(Error::Refused("egress_suspended".into()));
		}
		Ok(())
	}
	async fn debit(
		&self,
		scope: &ScopeSettings,
		direction: Direction,
		bytes: u64,
		day: NaiveDate,
	) -> Result<()> {
		self.check(&scope.id)?;
		let mut s = self.state.lock().unwrap();
		if s.suspended {
			return Err(Error::Refused("egress_suspended".into()));
		}
		let entry = s.usage.entry(day).or_default();
		let (counter, limit) = match direction {
			Direction::Sent => (&mut entry.0, self.settings.budget.sent_bytes),
			Direction::Received => (&mut entry.1, self.settings.budget.received_bytes),
		};
		let next = counter
			.checked_add(bytes)
			.filter(|next| *next <= limit)
			.ok_or_else(|| Error::Refused("egress_budget_exhausted".into()))?;
		*counter = next;
		Ok(())
	}
	async fn refund(
		&self,
		scope: &ScopeSettings,
		direction: Direction,
		bytes: u64,
		day: NaiveDate,
	) -> Result<()> {
		self.check(&scope.id)?;
		let mut s = self.state.lock().unwrap();
		let entry = s
			.usage
			.get_mut(&day)
			.ok_or_else(|| Error::Invalid("invalid refund".into()))?;
		let counter = match direction {
			Direction::Sent => &mut entry.0,
			Direction::Received => &mut entry.1,
		};
		*counter = counter
			.checked_sub(bytes)
			.ok_or_else(|| Error::Invalid("invalid refund".into()))?;
		Ok(())
	}
	async fn record(&self, record: ConnectionRecord) -> Result<()> {
		self.check(&record.scope)?;
		self.state.lock().unwrap().records.push(record);
		Ok(())
	}
	async fn lift_suspension(&self, scope: &ScopeId) -> Result<()> {
		self.check(scope)?;
		let mut s = self.state.lock().unwrap();
		s.suspended = false;
		s.window = Instant::now();
		s.connections = 0;
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidash_domain::egress::{AllowedHosts, AnomalyRule, DailyBudget};

	#[tokio::test]
	async fn credential_budget_and_suspension_are_scoped() {
		let settings = ScopeSettings {
			id: ScopeId {
				tenant: "tenant".into(),
				node: "node".into(),
				scratch_space: uuid::Uuid::new_v4(),
			},
			allowed_hosts: AllowedHosts::default(),
			budget: DailyBudget {
				sent_bytes: 10,
				received_bytes: 10,
			},
			anomaly: AnomalyRule {
				connections_per_minute: 1,
			},
			operator_allows_all: false,
		};
		let authority = MemoryScopes::new("token".into(), settings.clone());
		assert!(authority.resolve("wrong").await.is_err());
		let scope = authority.resolve("token").await.unwrap();
		let day = chrono::Utc::now().date_naive();
		authority
			.debit(&scope, Direction::Sent, 10, day)
			.await
			.unwrap();
		assert!(
			authority
				.debit(&scope, Direction::Sent, 1, day)
				.await
				.is_err()
		);
		authority
			.refund(&scope, Direction::Sent, 1, day)
			.await
			.unwrap();
		authority
			.debit(&scope, Direction::Sent, 1, day)
			.await
			.unwrap();
		authority.admit_connection(&scope).await.unwrap();
		assert!(authority.admit_connection(&scope).await.is_err());
		assert_eq!(authority.notifications(), 1);
		assert!(authority.admit_connection(&scope).await.is_err());
		authority.lift_suspension(&scope.id).await.unwrap();
		authority.admit_connection(&scope).await.unwrap();
	}
}
