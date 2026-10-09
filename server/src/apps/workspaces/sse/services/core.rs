//! Loss-tolerant UI wakeups. PostgreSQL, never this hub, owns events and replay.
#[path = "broker.rs"]
pub(crate) mod broker;
#[path = "connection.rs"]
pub(crate) mod connection;

use crate::{Error, Result};
use std::{
	collections::HashMap,
	sync::{
		Arc, Mutex, Weak,
		atomic::{AtomicBool, AtomicU64, Ordering},
	},
	time::Duration,
};
use tokio::sync::watch;
use uuid::Uuid;

pub use connection::EventStream;
pub(crate) use connection::StreamRequest;
pub(crate) use connection::inference::RunStreamRequest;

#[derive(Clone, Debug)]
pub struct Settings {
	pub reconcile_interval: Duration,
	pub backpressure_timeout: Duration,
}
impl Default for Settings {
	fn default() -> Self {
		Self {
			reconcile_interval: Duration::from_secs(5),
			backpressure_timeout: Duration::from_secs(30),
		}
	}
}
impl Settings {
	pub fn from_env() -> Result<Self> {
		Self::from_values(|name| std::env::var(name).ok())
	}
	fn from_values(value: impl Fn(&str) -> Option<String>) -> Result<Self> {
		let read = |name: &str, default: u64, min: u64, max: u64| -> Result<u64> {
			let parsed = value(name)
				.map(|v| v.parse::<u64>())
				.transpose()
				.map_err(|_| Error::Invalid(format!("invalid {name}")))?
				.unwrap_or(default);
			if !(min..=max).contains(&parsed) {
				return Err(Error::Invalid(format!("invalid {name}")));
			}
			Ok(parsed)
		};
		Ok(Self {
			reconcile_interval: Duration::from_millis(read(
				"AIDASH_SSE_RECONCILE_INTERVAL_MS",
				5000,
				250,
				60000,
			)?),
			backpressure_timeout: Duration::from_secs(read(
				"AIDASH_SSE_BACKPRESSURE_TIMEOUT_SECONDS",
				30,
				1,
				300,
			)?),
		})
	}
}

#[derive(Clone, Copy)]
pub(crate) struct Reasons(u8);
impl Reasons {
	pub const INITIAL: Self = Self(1);
	pub const NOTIFICATION: Self = Self(2);
	pub const RECONNECT: Self = Self(4);
	pub const FALLBACK: Self = Self(8);
	pub const BACKLOG: Self = Self(16);
	pub const GATE: Self = Self(32);
	fn add(&mut self, other: Self) {
		self.0 |= other.0;
	}
}
const REASONS: [&str; 6] = [
	"initial",
	"notification",
	"reconnect",
	"fallback",
	"backlog",
	"gate_release",
];

#[derive(Default)]
struct Counters {
	reads: AtomicU64,
	causes: [AtomicU64; 6],
	notifications: AtomicU64,
	rejected: AtomicU64,
	checks: AtomicU64,
	gate_checks: AtomicU64,
	gate_waiters: AtomicU64,
	frames: AtomicU64,
	backpressure: AtomicU64,
	ready: AtomicBool,
}

#[derive(Clone, Copy, Default)]
struct Generation {
	notification: u64,
	recovery: u64,
}
struct Slot {
	users: usize,
	signal: watch::Sender<Generation>,
}
struct Inner {
	settings: Settings,
	slots: Mutex<HashMap<Option<Uuid>, Slot>>,
	stopping: watch::Sender<bool>,
	counts: Counters,
}

#[derive(Clone)]
pub struct Service {
	inner: Arc<Inner>,
}
impl Service {
	pub fn new(settings: Settings) -> Self {
		metrics::gauge!("aidash_sse_reconcile_interval_seconds")
			.set(settings.reconcile_interval.as_secs_f64());
		metrics::gauge!("aidash_sse_backpressure_timeout_seconds")
			.set(settings.backpressure_timeout.as_secs_f64());
		Self {
			inner: Arc::new(Inner {
				settings,
				slots: Mutex::new(HashMap::new()),
				stopping: watch::channel(false).0,
				counts: Counters::default(),
			}),
		}
	}
	pub fn snapshot(&self) -> Snapshot {
		let c = &self.inner.counts;
		Snapshot {
			event_queries: c.reads.load(Ordering::Relaxed),
			query_causes: std::array::from_fn(|i| c.causes[i].load(Ordering::Relaxed)),
			notifications: c.notifications.load(Ordering::Relaxed),
			rejected_notifications: c.rejected.load(Ordering::Relaxed),
			authority_checks: c.checks.load(Ordering::Relaxed),
			gate_checks: c.gate_checks.load(Ordering::Relaxed),
			visibility_waiters: c.gate_waiters.load(Ordering::Relaxed),
			frames: c.frames.load(Ordering::Relaxed),
			backpressure_disconnects: c.backpressure.load(Ordering::Relaxed),
			transport_ready: c.ready.load(Ordering::Relaxed),
			registered_scopes: self.inner.slots.lock().expect("SSE registrations").len(),
		}
	}
	fn register(&self, scope: Option<Uuid>) -> Observation {
		let mut slots = self.inner.slots.lock().expect("SSE registrations");
		let slot = slots.entry(scope).or_insert_with(|| Slot {
			users: 0,
			signal: watch::channel(Generation::default()).0,
		});
		slot.users += 1;
		let receiver = slot.signal.subscribe();
		let seen = *receiver.borrow();
		metrics::gauge!("aidash_sse_registered_scopes").set(slots.len() as f64);
		Observation {
			hub: Arc::downgrade(&self.inner),
			scope,
			receiver,
			seen,
		}
	}
	fn notify(&self, workspace: Option<Uuid>, recovery: bool) {
		let slots = self.inner.slots.lock().expect("SSE registrations");
		for (scope, slot) in slots.iter() {
			if recovery || workspace.is_none() || scope.is_none() || *scope == workspace {
				slot.signal.send_modify(|v| {
					if recovery {
						v.recovery = v.recovery.wrapping_add(1);
					} else {
						v.notification = v.notification.wrapping_add(1);
					}
				});
			}
		}
	}
	fn record_read(&self, reasons: Reasons) {
		self.inner.counts.reads.fetch_add(1, Ordering::Relaxed);
		metrics::counter!("aidash_sse_event_queries_total").increment(1);
		for (index, label) in REASONS.into_iter().enumerate() {
			if reasons.0 & (1 << index) != 0 {
				self.inner.counts.causes[index].fetch_add(1, Ordering::Relaxed);
				metrics::counter!("aidash_sse_query_causes_total", "reason" => label).increment(1);
			}
		}
	}
	fn gate_check(&self) {
		self.inner
			.counts
			.gate_checks
			.fetch_add(1, Ordering::Relaxed);
		metrics::counter!("aidash_sse_visibility_checks_total").increment(1);
	}
	fn wait_for_gate(&self) -> GateWait {
		self.inner
			.counts
			.gate_waiters
			.fetch_add(1, Ordering::Relaxed);
		metrics::gauge!("aidash_sse_visibility_waiters").increment(1.0);
		GateWait(self.clone())
	}
	fn reconciled(&self) {
		metrics::gauge!("aidash_sse_last_reconciliation_timestamp_seconds").set(timestamp());
	}
}

/// Reader and body gate waits cannot overlap on one connection: the reader
/// waits for the body to finish its page. Drop also accounts for cancellation.
struct GateWait(Service);
impl Drop for GateWait {
	fn drop(&mut self) {
		self.0
			.inner
			.counts
			.gate_waiters
			.fetch_sub(1, Ordering::Relaxed);
		metrics::gauge!("aidash_sse_visibility_waiters").decrement(1.0);
	}
}

fn timestamp() -> f64 {
	std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs_f64()
}

struct Observation {
	hub: Weak<Inner>,
	scope: Option<Uuid>,
	receiver: watch::Receiver<Generation>,
	seen: Generation,
}
impl Observation {
	fn take_changes(&mut self) -> Reasons {
		let current = *self.receiver.borrow_and_update();
		let mut reasons = Reasons(0);
		if current.notification != self.seen.notification {
			reasons.add(Reasons::NOTIFICATION);
			let coalesced = current
				.notification
				.wrapping_sub(self.seen.notification)
				.saturating_sub(1);
			metrics::counter!("aidash_sse_coalesced_wakeups_total").increment(coalesced);
		}
		if current.recovery != self.seen.recovery {
			reasons.add(Reasons::RECONNECT);
		}
		self.seen = current;
		reasons
	}
}
impl Drop for Observation {
	fn drop(&mut self) {
		if let Some(hub) = self.hub.upgrade() {
			let mut slots = hub.slots.lock().expect("SSE registrations");
			if let Some(slot) = slots.get_mut(&self.scope) {
				slot.users -= 1;
				if slot.users == 0 {
					slots.remove(&self.scope);
				}
			}
			metrics::gauge!("aidash_sse_registered_scopes").set(slots.len() as f64);
		}
	}
}

// Preserve a statement before the value until reinhardt-web#6441 is fixed.
#[injectable(scope = "singleton")]
pub async fn provide() -> Service {
	tracing::trace!(service = "Service", "creating injectable service");
	Service::new(Settings::default())
}

use reinhardt::injectable;

pub use crate::apps::workspaces::sse::serializers::core::Snapshot;

#[cfg(test)]
#[path = "../tests/services_core_tests.rs"]
mod tests;
