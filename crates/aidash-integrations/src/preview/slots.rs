//! The Preview gateway's connection slots (#780). A connection nobody has been
//! admitted on yet holds one of a few shared slots, so a client that never
//! signs in can only fill those. Once a request on it is admitted, it moves to
//! a slot of that preview's own, under a ceiling on all admitted connections,
//! and a WebSocket tunnel keeps that slot until it closes.
use std::{
	collections::HashMap,
	sync::{Arc, Mutex},
	time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Connections not yet admitted, shared by every client.
const UNIDENTIFIED_SLOTS: usize = 64;
/// Admitted connections and tunnels across all previews.
const ADMITTED_SLOTS: usize = 512;
/// Admitted connections and tunnels of one preview: a few viewers with a few
/// tabs each, every tab with up to six connections plus its hot reload socket.
const PREVIEW_SLOTS: usize = 32;
/// Anyone can fill the unidentified slots, so the log is thinned. The counter
/// stays exact.
const REFUSAL_LOG_INTERVAL: Duration = Duration::from_secs(10);

/// Why a connection or request was turned away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Refusal {
	/// The unidentified slots or the admitted ceiling are full.
	Busy,
	/// This preview's own slots are full.
	PreviewFull,
	/// A request named another preview than the one its connection is held for.
	OtherPreview,
}

impl Refusal {
	pub(super) fn code(self) -> &'static str {
		match self {
			Self::Busy => "preview_gateway_busy",
			Self::PreviewFull => "preview_connection_limit",
			Self::OtherPreview => "preview_misdirected",
		}
	}
}

pub(super) struct Slots {
	unidentified: Arc<Semaphore>,
	admitted: Arc<Semaphore>,
	per_preview: usize,
	previews: Mutex<HashMap<String, Arc<Semaphore>>>,
	refused: Mutex<RefusedConnections>,
}

impl Default for Slots {
	fn default() -> Self {
		Self::with_sizes(UNIDENTIFIED_SLOTS, ADMITTED_SLOTS, PREVIEW_SLOTS)
	}
}

impl Slots {
	pub(super) fn with_sizes(unidentified: usize, admitted: usize, per_preview: usize) -> Self {
		Self {
			unidentified: Arc::new(Semaphore::new(unidentified)),
			admitted: Arc::new(Semaphore::new(admitted)),
			per_preview,
			previews: Mutex::default(),
			refused: Mutex::default(),
		}
	}

	/// A slot for a connection just accepted, if one is free.
	pub(super) fn arrive(&self) -> Option<Connection> {
		let permit = self.unidentified.clone().try_acquire_owned().ok()?;
		Some(Connection(Mutex::new(Held::Unidentified { _slot: permit })))
	}

	/// Moves `connection` to the slot of the preview at `host`, once a request
	/// on it has been admitted there. Its unidentified slot is given back.
	pub(super) fn admit(&self, connection: &Connection, host: &str) -> Result<(), Refusal> {
		let host = preview_key(host);
		let mut held = connection.0.lock().unwrap_or_else(|e| e.into_inner());
		match &*held {
			Held::Admitted(admitted) if admitted.host == host => return Ok(()),
			Held::Admitted(_) => return Err(Refusal::OtherPreview),
			// Upgraded: no further request arrives on this connection.
			Held::Tunnelled => return Ok(()),
			Held::Unidentified { .. } => {}
		}
		let Ok(total) = self.admitted.clone().try_acquire_owned() else {
			return Err(Refusal::Busy);
		};
		let preview = {
			let mut previews = self.previews.lock().unwrap_or_else(|e| e.into_inner());
			// A permit holds its semaphore, so one no permit holds is unused.
			previews.retain(|_, slots| Arc::strong_count(slots) > 1);
			previews
				.entry(host.clone())
				.or_insert_with(|| Arc::new(Semaphore::new(self.per_preview)))
				.clone()
		};
		let Ok(own) = preview.try_acquire_owned() else {
			return Err(Refusal::PreviewFull);
		};
		*held = Held::Admitted(Admitted {
			host,
			_total: total,
			_own: own,
		});
		Ok(())
	}

	/// Counts a refusal and logs it at most once per interval.
	pub(super) fn note(&self, refusal: Refusal, outcome: &'static str) {
		let slots = match refusal {
			Refusal::Busy => "busy",
			Refusal::PreviewFull => "preview",
			Refusal::OtherPreview => return,
		};
		metrics::counter!(
			"aidash_preview_connections_refused_total",
			"slots" => slots,
			"outcome" => outcome
		)
		.increment(1);
		let mut refused = self.refused.lock().unwrap_or_else(|e| e.into_inner());
		refused.unlogged += 1;
		if refused
			.logged_at
			.is_none_or(|at| at.elapsed() >= REFUSAL_LOG_INTERVAL)
		{
			tracing::warn!(
				refused = refused.unlogged,
				code = refusal.code(),
				"Preview connection slots are full"
			);
			refused.unlogged = 0;
			refused.logged_at = Some(Instant::now());
		}
	}
}

/// Previews are told apart by subdomain; a port or letter case does not make
/// another one.
fn preview_key(host: &str) -> String {
	let name = match host.rsplit_once(':') {
		Some((name, port)) if port.bytes().all(|b| b.is_ascii_digit()) => name,
		_ => host,
	};
	name.trim_end_matches('.').to_ascii_lowercase()
}

#[derive(Default)]
struct RefusedConnections {
	unlogged: u64,
	logged_at: Option<Instant>,
}

/// The slot one accepted connection holds.
pub(super) struct Connection(Mutex<Held>);

enum Held {
	Unidentified { _slot: OwnedSemaphorePermit },
	Admitted(Admitted),
	Tunnelled,
}

/// An admitted connection's slots. Dropping it gives them back.
pub(super) struct Admitted {
	host: String,
	_total: OwnedSemaphorePermit,
	_own: OwnedSemaphorePermit,
}

impl Connection {
	/// Hands the connection's admitted slot to the WebSocket tunnel it becomes,
	/// which outlives the HTTP connection.
	pub(super) fn tunnel(&self) -> Option<Admitted> {
		let mut held = self.0.lock().unwrap_or_else(|e| e.into_inner());
		match std::mem::replace(&mut *held, Held::Tunnelled) {
			Held::Admitted(admitted) => Some(admitted),
			other => {
				*held = other;
				None
			}
		}
	}
}

#[cfg(test)]
mod tests;
