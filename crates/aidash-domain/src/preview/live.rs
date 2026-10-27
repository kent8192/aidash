//! A Live preview: one Run's Web server, pinned to a Working Area revision,
//! which keeps serving only inside its idle and absolute limits (ADR 0070).
//!
//! The rules here are free of storage, clocks and transports: the Runner starts
//! the Pod, the server persists the facts and the Preview gateway refreshes the
//! idle timer, and both ask these functions what the state means.
use crate::{Error, Result};
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The Deployment's default idle limit: 15 minutes without a gateway request.
pub const DEFAULT_IDLE_SECONDS: i64 = 900;
/// The Deployment's default absolute limit: 2 hours from start.
pub const DEFAULT_ABSOLUTE_SECONDS: i64 = 7200;
/// How long a start may wait for the declared port to accept connections.
pub const START_SECONDS: i64 = 120;
/// The shortest limit a Deployment may configure, so a start is never useless.
pub const MINIMUM_SECONDS: i64 = 60;
/// The longest absolute limit a Deployment may configure.
pub const MAXIMUM_ABSOLUTE_SECONDS: i64 = 24 * 3600;
/// A Pod runs as an unprivileged user, so it cannot bind a privileged port.
pub const MINIMUM_PORT: u16 = 1024;
/// The bounded startup log tail a start returns.
pub const MAX_STARTUP_LOG_BYTES: usize = 16 * 1024;

/// The idle and absolute lifetimes of a Live preview. A Tenant may only lower
/// the Deployment's ceilings, as with Test Limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
	pub idle_seconds: i64,
	pub absolute_seconds: i64,
}

impl Default for Limits {
	fn default() -> Self {
		Self {
			idle_seconds: DEFAULT_IDLE_SECONDS,
			absolute_seconds: DEFAULT_ABSOLUTE_SECONDS,
		}
	}
}

impl Limits {
	/// Validate one set of limits: both positive, idle no longer than absolute.
	pub fn new(idle_seconds: i64, absolute_seconds: i64) -> Result<Self> {
		if idle_seconds < MINIMUM_SECONDS
			|| absolute_seconds < MINIMUM_SECONDS
			|| idle_seconds > absolute_seconds
			|| absolute_seconds > MAXIMUM_ABSOLUTE_SECONDS
		{
			return Err(Error::Invalid(
				"live preview limits must be positive and idle no longer than absolute".into(),
			));
		}
		Ok(Self {
			idle_seconds,
			absolute_seconds,
		})
	}

	/// A Tenant's values, lowered to the Deployment's ceilings. A Tenant cannot
	/// raise either one, and the idle limit never outlives the absolute one.
	pub fn lower_than(&self, ceiling: &Self) -> Result<Self> {
		let idle_seconds = self.idle_seconds.min(ceiling.idle_seconds);
		let absolute_seconds = self.absolute_seconds.min(ceiling.absolute_seconds);
		Self::new(idle_seconds.min(absolute_seconds), absolute_seconds)
	}

	/// Whether `self` stays within `ceiling`.
	pub fn within(&self, ceiling: &Self) -> bool {
		self.idle_seconds <= ceiling.idle_seconds
			&& self.absolute_seconds <= ceiling.absolute_seconds
	}

	pub fn idle(&self) -> TimeDelta {
		TimeDelta::seconds(self.idle_seconds)
	}

	pub fn absolute(&self) -> TimeDelta {
		TimeDelta::seconds(self.absolute_seconds)
	}
}

/// What a Live preview is doing now, as an operator reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
	/// A start is being dispatched: it has no Pod it can serve from yet.
	Starting,
	/// Its Pod is serving the revision it pinned.
	Running,
	/// It still serves its pinned revision, but a later publication has left it
	/// behind; a later turn restarts it. Visible, not stopped.
	Stale,
	/// It is not serving: the process stopped, or a stop is in progress.
	Stopped,
}

impl State {
	pub fn as_str(&self) -> &'static str {
		match self {
			Self::Starting => "starting",
			Self::Running => "running",
			Self::Stale => "stale",
			Self::Stopped => "stopped",
		}
	}

	pub fn parse(value: &str) -> Result<Self> {
		match value {
			"starting" => Ok(Self::Starting),
			"running" => Ok(Self::Running),
			"stale" => Ok(Self::Stale),
			"stopped" => Ok(Self::Stopped),
			_ => Err(Error::UnsupportedState(format!(
				"unknown live preview state {value}"
			))),
		}
	}
}

/// Why a Live preview stopped. A person reading the status sees the first four;
/// the rest explain a stop they did not ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
	/// A person or the Run itself asked to stop it.
	Stopped,
	/// Starting another Live preview replaced it (at most one per Run).
	Replaced,
	/// The Run was cancelled.
	RunCancelled,
	/// The Run's authority no longer admits it.
	RunAuthorityRevoked,
	/// Its Tenant is being deleted.
	TenantDeleted,
	/// Its Working Area was cleaned up.
	AreaCleanedUp,
	/// No gateway request within the idle limit.
	Idle,
	/// Reached its absolute limit.
	Absolute,
	/// The Web server never accepted connections, or the Pod died.
	StartFailed,
}

impl StopReason {
	pub fn as_str(&self) -> &'static str {
		match self {
			Self::Stopped => "stopped",
			Self::Replaced => "replaced",
			Self::RunCancelled => "run_cancelled",
			Self::RunAuthorityRevoked => "run_authority_revoked",
			Self::TenantDeleted => "tenant_deleted",
			Self::AreaCleanedUp => "area_cleaned_up",
			Self::Idle => "idle",
			Self::Absolute => "absolute",
			Self::StartFailed => "start_failed",
		}
	}

	pub fn parse(value: &str) -> Result<Self> {
		match value {
			"stopped" => Ok(Self::Stopped),
			"replaced" => Ok(Self::Replaced),
			"run_cancelled" => Ok(Self::RunCancelled),
			"run_authority_revoked" => Ok(Self::RunAuthorityRevoked),
			"tenant_deleted" => Ok(Self::TenantDeleted),
			"area_cleaned_up" => Ok(Self::AreaCleanedUp),
			"idle" => Ok(Self::Idle),
			"absolute" => Ok(Self::Absolute),
			"start_failed" => Ok(Self::StartFailed),
			_ => Err(Error::UnsupportedState(format!(
				"unknown live preview stop reason {value}"
			))),
		}
	}
}

/// One persisted Live preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivePreview {
	pub id: Uuid,
	pub tenant: String,
	pub run_id: Uuid,
	pub area_id: Uuid,
	/// The Working Area revision the Pod mounted; never changes while it serves.
	pub revision: i64,
	/// The port the Pod declares and the Preview gateway reaches.
	pub port: u16,
	pub state: State,
	pub stop_reason: Option<StopReason>,
	pub limits: Limits,
	pub started_at: DateTime<Utc>,
	pub last_request_at: DateTime<Utc>,
	/// The absolute deadline, persisted so a restart still stops it.
	pub expires_at: DateTime<Utc>,
	/// Where the gateway reaches the Pod; `None` until the Runner reports it.
	pub pod_host: Option<String>,
	/// Whether the Runner proved the Pod is gone.
	pub termination_confirmed: bool,
	pub egress_expires_at: Option<DateTime<Utc>>,
}

impl LivePreview {
	/// A row for a start that is being dispatched. The state stays `Starting`
	/// until the Runner reports the port is accepting, so a crash mid-start
	/// leaves nothing claiming to serve; a sweep ends it after the start
	/// deadline.
	#[allow(clippy::too_many_arguments)]
	pub fn starting(
		id: Uuid,
		tenant: impl Into<String>,
		run_id: Uuid,
		area_id: Uuid,
		revision: i64,
		port: u16,
		limits: Limits,
		now: DateTime<Utc>,
	) -> Result<Self> {
		let port = requested_port(port)?;
		Ok(Self {
			id,
			tenant: tenant.into(),
			run_id,
			area_id,
			revision,
			port,
			state: State::Starting,
			stop_reason: None,
			started_at: now,
			last_request_at: now,
			expires_at: now + limits.absolute(),
			limits,
			pod_host: None,
			termination_confirmed: true,
			egress_expires_at: None,
		})
	}

	/// The Runner's declared port accepts connections: now it serves.
	pub fn serving(&mut self, pod_host: impl Into<String>, now: DateTime<Utc>) {
		self.pod_host = Some(pod_host.into());
		self.state = State::Running;
		self.stop_reason = None;
		self.started_at = now;
		self.last_request_at = now;
		self.expires_at = now + self.limits.absolute();
		self.termination_confirmed = false;
	}

	/// The state as a reader sees it now: a later revision makes a running
	/// preview stale without stopping it (it still serves the pinned revision).
	pub fn view(&self, area_revision: i64) -> State {
		match self.state {
			State::Running if area_revision > self.revision => State::Stale,
			state => state,
		}
	}

	/// Whether a start that never became ready has exhausted its wait. The
	/// dispatcher waits [`START_SECONDS`], plus a margin for a slow restart.
	pub fn start_expired(&self, now: DateTime<Utc>) -> bool {
		self.state == State::Starting
			&& now >= self.started_at + TimeDelta::seconds(START_SECONDS + 30)
	}

	pub fn running(&self) -> bool {
		matches!(self.state, State::Running | State::Stale)
	}

	/// A gateway request keeps it alive, but never past its absolute deadline.
	pub fn touched(&mut self, now: DateTime<Utc>) {
		if self.running() {
			self.last_request_at = now;
		}
	}

	/// The moment its idle limit would expire; `None` once it stopped.
	pub fn idle_deadline(&self) -> Option<DateTime<Utc>> {
		self.running()
			.then(|| self.last_request_at + self.limits.idle())
	}

	/// Whether a gate request is still admitted.
	pub fn admits(&self, now: DateTime<Utc>) -> bool {
		self.running() && now < self.expires_at && now < self.last_request_at + self.limits.idle()
	}

	/// The reason it must now stop by itself, if any.
	pub fn expired(&self, now: DateTime<Utc>) -> Option<StopReason> {
		if !self.running() {
			return None;
		}
		if now >= self.expires_at {
			return Some(StopReason::Absolute);
		}
		if now >= self.last_request_at + self.limits.idle() {
			return Some(StopReason::Idle);
		}
		None
	}

	/// Begin stopping; it stops serving at once and the hold stays until the
	/// Runner proves the Pod is gone.
	pub fn stopping(&mut self, reason: StopReason) {
		self.state = State::Stopped;
		self.stop_reason = Some(reason);
		self.termination_confirmed = false;
	}

	/// The Runner proved the Pod stopped: the row is settled.
	pub fn stopped(&mut self, reason: StopReason) {
		self.state = State::Stopped;
		self.stop_reason = Some(reason);
		self.termination_confirmed = true;
	}

	/// A stop that must still reach the Runner before the row is settled.
	pub fn needs_termination(&self) -> bool {
		self.state.is_stopped() && !self.termination_confirmed
	}
}

impl State {
	pub fn is_stopped(self) -> bool {
		self == Self::Stopped
	}
}

fn requested_port(port: u16) -> Result<u16> {
	if port < MINIMUM_PORT {
		return Err(Error::Invalid(
			"a Live preview must declare a non-privileged port".into(),
		));
	}
	Ok(port)
}

/// A bounded startup log tail: the last bytes the Runner's Pod wrote, on a
/// character boundary, so a model sees why a start failed without a dump.
pub fn startup_tail(log: &str) -> String {
	let bytes = log.as_bytes();
	if bytes.len() <= MAX_STARTUP_LOG_BYTES {
		return log.to_owned();
	}
	let start = bytes.len() - MAX_STARTUP_LOG_BYTES;
	let start = (start..bytes.len())
		.find(|index| std::str::from_utf8(&bytes[*index..]).is_ok())
		.unwrap_or(bytes.len());
	String::from_utf8_lossy(&bytes[start..]).into_owned()
}

#[cfg(test)]
mod tests;
