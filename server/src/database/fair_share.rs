//! Admission for one process's Work pool.
use aidash_domain::tenancy::TenantName;
use reinhardt::core::exception::{DatabaseError, DatabaseErrorKind, Error};
use std::{
	collections::{HashMap, VecDeque},
	sync::{
		Arc, Mutex,
		atomic::{AtomicUsize, Ordering},
	},
	time::Duration,
};
use tokio::sync::{Notify, OnceCell};

// Wait at most as long as a PostgreSQL pool acquire before reporting pressure.
pub(super) const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const FAIR_SHARE_CODE: &str = "database_fair_share";
pub(crate) const POOL_BUSY_CODE: &str = "database_pool_busy";

tokio::task_local! {
	static OPERATION: Arc<Operation>;
}

pub(super) fn pool_busy(name: &str, size: usize) -> Error {
	DatabaseError::new(
		DatabaseErrorKind::Timeout,
		format!("The {name} database pool is busy ({size} connections). Retry."),
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

struct Operation {
	pool: Arc<FairShare>,
	tenant: Option<TenantName>,
	admitted: OnceCell<()>,
	// Updated under the pool state lock so tenant counts and usage move together.
	borrows: AtomicUsize,
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
				// Include the incoming operation when checking whether it would exceed its share.
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
		if self.used >= size {
			return None;
		}
		self.lanes
			.iter()
			.find(|lane| !self.over_share(lane.tenant.as_ref(), size))
			.and_then(|lane| lane.tickets.front().copied())
	}

	fn borrowed(
		&mut self,
		tenant: Option<&TenantName>,
		operation: Option<&Operation>,
		size: usize,
	) {
		self.used += 1;
		if operation.is_none_or(|op| op.borrows.fetch_add(1, Ordering::Relaxed) == 0)
			&& let Some(tenant) = tenant
		{
			*self.holders.entry(tenant.clone()).or_default() += 1;
		}
		self.update_pressure(size);
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

	pub(super) async fn operation<T>(
		self: &Arc<Self>,
		tenant: Option<TenantName>,
		work: impl Future<Output = T>,
	) -> T {
		if self.current_operation(tenant.as_ref()).is_some() {
			return work.await;
		}
		OPERATION
			.scope(
				Arc::new(Operation {
					pool: self.clone(),
					tenant,
					admitted: OnceCell::new(),
					borrows: AtomicUsize::new(0),
				}),
				work,
			)
			.await
	}

	fn current_operation(self: &Arc<Self>, tenant: Option<&TenantName>) -> Option<Arc<Operation>> {
		OPERATION
			.try_with(|op| {
				(Arc::ptr_eq(self, &op.pool) && (tenant.is_none() || op.tenant.as_ref() == tenant))
					.then(|| op.clone())
			})
			.ok()
			.flatten()
	}

	pub(super) async fn borrow(self: &Arc<Self>) -> Result<Permit, Error> {
		let Some(op) = self.current_operation(None) else {
			return self.acquire(None, None).await;
		};
		let mut first = None;
		op.admitted
			.get_or_try_init(|| async {
				first = Some(self.acquire(op.tenant.clone(), Some(op.clone())).await?);
				Ok::<_, Error>(())
			})
			.await?;
		if let Some(permit) = first {
			return Ok(permit);
		}
		// Continuations still occupy the driver pool and contribute to pressure.
		// Reentering the admission queue here could wait on our own operation.
		self.state
			.lock()
			.expect("database admission state")
			.borrowed(op.tenant.as_ref(), Some(&op), self.size);
		Ok(Permit {
			pool: self.clone(),
			tenant: op.tenant.clone(),
			operation: Some(op),
		})
	}

	async fn wait(
		self: &Arc<Self>,
		waiter: &Waiter<'_>,
		operation: Option<Arc<Operation>>,
	) -> Permit {
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
					state.last = Some(waiter.tenant.clone());
					state.borrowed(waiter.tenant.as_ref(), operation.as_deref(), self.size);
					self.changed.notify_waiters();
					return Permit {
						pool: self.clone(),
						tenant: waiter.tenant.clone(),
						operation,
					};
				}
			}
			changed.await;
		}
	}

	async fn acquire(
		self: &Arc<Self>,
		tenant: Option<TenantName>,
		operation: Option<Arc<Operation>>,
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
		match tokio::time::timeout(ACQUIRE_TIMEOUT, self.wait(&waiter, operation)).await {
			Ok(permit) => Ok(permit),
			Err(_) => {
				let state = self.state.lock().expect("database admission state");
				if state.over_share(waiter.tenant.as_ref(), self.size) {
					let share = state.share(self.size);
					Err(DatabaseError::new(
						DatabaseErrorKind::Timeout,
						format!(
							"Database fair share wait expired. The Work pool has {} connections and this Tenant's share is {share} operations.",
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
	operation: Option<Arc<Operation>>,
}

impl Drop for Permit {
	fn drop(&mut self) {
		let mut state = self.pool.state.lock().expect("database admission state");
		state.used -= 1;
		if self
			.operation
			.as_ref()
			.is_none_or(|op| op.borrows.fetch_sub(1, Ordering::Relaxed) == 1)
			&& let Some(tenant) = &self.tenant
		{
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
