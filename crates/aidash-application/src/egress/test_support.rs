//! An in-memory scope authority for gateway tests, never for deployment.
use super::{ConnectionRecord, Direction, EgressScopes, RefusalTally, ScopeSettings};
use crate::{Error, Result};
use aidash_domain::egress::ScopeId;
use async_trait::async_trait;
use chrono::NaiveDate;
use std::{collections::HashMap, sync::Mutex};

struct State {
	revoked: bool,
	notifications: u32,
	refusals: Vec<RefusalTally>,
	checks: u32,
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
				revoked: false,
				notifications: 0,
				refusals: Vec::new(),
				checks: 0,
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
	/// Makes the credential stop authenticating, as a deleted Space's does.
	pub fn revoke(&self) {
		self.state.lock().unwrap().revoked = true;
	}
	pub fn refusals(&self) -> Vec<RefusalTally> {
		self.state.lock().unwrap().refusals.clone()
	}
	pub fn checks(&self) -> u32 {
		self.state.lock().unwrap().checks
	}
	pub fn usage(&self, day: NaiveDate) -> (u64, u64) {
		self.state
			.lock()
			.unwrap()
			.usage
			.get(&day)
			.copied()
			.unwrap_or_default()
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
		if !super::credentials_match(&self.credential, credential)
			|| self.state.lock().unwrap().revoked
		{
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
		let s = self.state.lock().unwrap();
		let usage = s
			.usage
			.get(&chrono::Utc::now().date_naive())
			.copied()
			.unwrap_or_default();
		if usage.0 >= self.settings.budget.sent_bytes
			|| usage.1 >= self.settings.budget.received_bytes
		{
			return Err(Error::Refused("egress_budget_exhausted".into()));
		}
		Ok(())
	}
	async fn check_active(&self, scope: &ScopeSettings, credential: &str) -> Result<ScopeSettings> {
		let current = self.resolve(credential).await?;
		self.check(&scope.id)?;
		let mut state = self.state.lock().unwrap();
		state.checks += 1;
		Ok(current)
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
		let mut state = self.state.lock().unwrap();
		if let Some(existing) = state
			.records
			.iter_mut()
			.find(|existing| existing.id == record.id)
		{
			*existing = record;
		} else {
			state.records.push(record);
		}
		Ok(())
	}
	async fn record_refusals(&self, tallies: Vec<RefusalTally>) -> Result<()> {
		for tally in &tallies {
			self.check(&tally.scope)?;
		}
		self.state.lock().unwrap().refusals.extend(tallies);
		Ok(())
	}
	async fn notify_many_hosts(&self, scope: &ScopeId, _: usize) -> Result<()> {
		self.check(scope)?;
		self.state.lock().unwrap().notifications += 1;
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidash_domain::egress::{AllowedHosts, AnomalyRule, DailyBudget};

	#[test]
	fn credential_comparison_rejects_mismatches() {
		assert!(super::super::credentials_match(
			"fixed-token",
			"fixed-token"
		));
		assert!(!super::super::credentials_match(
			"fixed-token",
			"fixed-tokem"
		));
		assert!(!super::super::credentials_match("fixed-token", "short"));
	}

	#[tokio::test]
	async fn credential_and_budget_are_scoped() {
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
			anomaly: AnomalyRule::default(),
			operator_allows_all: false,
			tunnel_limits: Default::default(),
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
		assert!(
			authority
				.admit_connection(&scope)
				.await
				.unwrap_err()
				.to_string()
				.contains("egress_budget_exhausted")
		);
		authority
			.refund(&scope, Direction::Sent, 1, day)
			.await
			.unwrap();
		authority
			.debit(&scope, Direction::Sent, 1, day)
			.await
			.unwrap();
		authority
			.refund(&scope, Direction::Sent, 1, day)
			.await
			.unwrap();
		authority.admit_connection(&scope).await.unwrap();
		authority.admit_connection(&scope).await.unwrap();
	}
}
