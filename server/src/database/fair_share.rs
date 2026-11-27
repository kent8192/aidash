//! Admission for one process's Work pool.
use aidash_domain::tenancy::TenantName;
use reinhardt::core::exception::{DatabaseError, DatabaseErrorKind, Error};
use std::{
	collections::{HashMap, VecDeque},
	sync::{Arc, Mutex},
	time::Duration,
};
use tokio::sync::Notify;

// Wait at most as long as a PostgreSQL pool acquire before reporting pressure.
pub(super) const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const FAIR_SHARE_CODE: &str = "database_fair_share";
pub(crate) const POOL_BUSY_CODE: &str = "database_pool_busy";

tokio::task_local! {
	pub(super) static TENANT: Option<TenantName>;
}

pub(super) fn pool_busy(name: &str, size: usize) -> Error {
	DatabaseError::new(
		DatabaseErrorKind::Timeout,
		format!("The {name} database pool is busy ({size} connections); retry"),
	)
	.with_code(POOL_BUSY_CODE)
	.into()
}

pub(super) struct FairShare {
	size: usize,
	state: Mutex<State>,
	changed: Notify,
}

#[derive(Default)]
struct State {
	used: usize,
	pressured: bool,
	holders: HashMap<TenantName, usize>,
	lanes: VecDeque<Lane>,
	last: Option<Option<TenantName>>,
	next_ticket: u64,
}

struct Lane {
	tenant: Option<TenantName>,
	tickets: VecDeque<u64>,
}

impl State {
	fn share(&self, size: usize) -> usize {
		(size / self.holders.len().max(1)).max(1)
	}

	fn over_share(&self, tenant: Option<&TenantName>, size: usize) -> bool {
		self.pressured
			&& tenant.is_some_and(|tenant| {
				// Include this borrow when checking whether it would exceed its share.
				self.holders.get(tenant).copied().unwrap_or(0) >= self.share(size)
			})
	}

	fn update_pressure(&mut self, size: usize) {
		// Hysteresis keeps admission from switching modes around one threshold.
		if self.used * 100 > size * 80 {
			self.pressured = true;
		} else if self.used * 100 < size * 70 {
			self.pressured = false;
		}
	}

	fn next(&self, size: usize) -> Option<u64> {
		if self.used == size {
			return None;
		}
		self.lanes
			.iter()
			.find(|lane| !self.over_share(lane.tenant.as_ref(), size))
			.and_then(|lane| lane.tickets.front().copied())
	}

	fn remove(&mut self, ticket: u64, tenant: &Option<TenantName>) {
		if let Some(index) = self.lanes.iter().position(|lane| &lane.tenant == tenant) {
			let tickets = &mut self.lanes[index].tickets;
			tickets.retain(|queued| *queued != ticket);
			if tickets.is_empty() {
				self.lanes.remove(index);
			}
		}
	}
}

impl FairShare {
	pub(super) fn new(size: usize) -> Self {
		Self {
			size,
			state: Mutex::new(State::default()),
			changed: Notify::new(),
		}
	}

	async fn wait(self: &Arc<Self>, waiter: &Waiter) -> Permit {
		loop {
			let changed = self.changed.notified();
			tokio::pin!(changed);
			changed.as_mut().enable();
			{
				let mut state = self.state.lock().expect("database admission state");
				if state.next(self.size) == Some(waiter.ticket) {
					let index = state
						.lanes
						.iter()
						.position(|lane| lane.tenant == waiter.tenant)
						.expect("admitted lane is queued");
					let mut lane = state.lanes.remove(index).expect("admitted lane");
					lane.tickets.pop_front();
					if !lane.tickets.is_empty() {
						state.lanes.push_back(lane);
					}
					state.used += 1;
					state.last = Some(waiter.tenant.clone());
					if let Some(tenant) = &waiter.tenant {
						*state.holders.entry(tenant.clone()).or_default() += 1;
					}
					state.update_pressure(self.size);
					self.changed.notify_waiters();
					return Permit {
						pool: self.clone(),
						tenant: waiter.tenant.clone(),
					};
				}
			}
			changed.await;
		}
	}

	pub(super) async fn acquire(
		self: &Arc<Self>,
		tenant: Option<TenantName>,
	) -> Result<Permit, Error> {
		let ticket = {
			let mut state = self.state.lock().expect("database admission state");
			let ticket = state.next_ticket;
			state.next_ticket += 1;
			if let Some(lane) = state.lanes.iter_mut().find(|lane| lane.tenant == tenant) {
				lane.tickets.push_back(ticket);
			} else {
				let lane = Lane {
					tenant: tenant.clone(),
					tickets: VecDeque::from([ticket]),
				};
				// Let newly waiting Tenants go before the lane that just
				// received a slot, even if it queued again while full.
				let before_last = state
					.lanes
					.back()
					.is_some_and(|back| state.last.as_ref() == Some(&back.tenant));
				let index = state.lanes.len() - usize::from(before_last);
				state.lanes.insert(index, lane);
			}
			ticket
		};
		let waiter = Waiter {
			pool: self,
			tenant,
			ticket,
		};
		match tokio::time::timeout(ACQUIRE_TIMEOUT, self.wait(&waiter)).await {
			Ok(permit) => Ok(permit),
			Err(_) => {
				let state = self.state.lock().expect("database admission state");
				if state.over_share(waiter.tenant.as_ref(), self.size) {
					let share = state.share(self.size);
					Err(DatabaseError::new(
						DatabaseErrorKind::Timeout,
						format!(
							"Database fair share wait expired (work pool: {} connections, Tenant share: {share}); retry",
							self.size
						),
					)
					.with_code(FAIR_SHARE_CODE)
					.into())
				} else {
					Err(pool_busy("work", self.size))
				}
			}
		}
	}
}

struct Waiter<'a> {
	pool: &'a FairShare,
	tenant: Option<TenantName>,
	ticket: u64,
}

impl Drop for Waiter<'_> {
	fn drop(&mut self) {
		self.pool
			.state
			.lock()
			.expect("database admission state")
			.remove(self.ticket, &self.tenant);
		self.pool.changed.notify_waiters();
	}
}

pub(super) struct Permit {
	pool: Arc<FairShare>,
	tenant: Option<TenantName>,
}

impl Drop for Permit {
	fn drop(&mut self) {
		let mut state = self.pool.state.lock().expect("database admission state");
		state.used -= 1;
		if let Some(tenant) = &self.tenant {
			let count = state.holders.get_mut(tenant).expect("admitted Tenant");
			*count -= 1;
			if *count == 0 {
				state.holders.remove(tenant);
			}
		}
		state.update_pressure(self.pool.size);
		self.pool.changed.notify_waiters();
	}
}

#[cfg(test)]
mod tests;
