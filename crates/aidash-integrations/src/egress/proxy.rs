//! Scope-authenticated forward proxy.

use aidash_application::egress::{Direction, EgressScopes, ScopeSettings};
use http_body_util::BodyExt;
use hyper::body::Body as _;
use std::{
	io,
	sync::{
		Arc,
		atomic::{AtomicU64, Ordering},
	},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const BATCH: u64 = 1024 * 1024;
/// A Pod can repeat these failures at will, by reaching a host that always fails
/// or by opening connections during a stall, so each kind logs at most one line
/// per interval. The rest go to `debug`, and the counters stay exact.
const FAILURE_LOG_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
/// One last try at the final record once the process is stopping. With a refund
/// of up to 5 s each way before it, this stays inside the 20 s application drain.
const STOPPING_ATTEMPT: std::time::Duration = std::time::Duration::from_secs(5);
/// A live connection, or one whose final record was never written.
const UNSETTLED: &str = "unsettled";
const INVALID_TARGET: &str = "invalid_proxy_target";
const WRITE_FAILED: &str = "write failed";
/// The client reset or stopped reading. A tunnel cannot see inside TLS, so this
/// may be a finished download or an abandoned one, and it is no failure of ours.
const CLIENT_CLOSED: &str = "egress_client_closed";

/// Failures of each kind since that kind's last log line.
#[derive(Default)]
struct FailureLog {
	kinds: std::sync::Mutex<std::collections::HashMap<&'static str, ThinnedLine>>,
}
#[derive(Default)]
struct ThinnedLine {
	unlogged: u64,
	logged_at: Option<tokio::time::Instant>,
}
impl FailureLog {
	/// The number of failures a line should report, when one is due.
	fn due(&self, kind: &'static str) -> Option<u64> {
		let mut kinds = self.kinds.lock().unwrap();
		let line = kinds.entry(kind).or_default();
		line.unlogged += 1;
		if line
			.logged_at
			.is_some_and(|at| at.elapsed() < FAILURE_LOG_INTERVAL)
		{
			return None;
		}
		line.logged_at = Some(tokio::time::Instant::now());
		Some(std::mem::take(&mut line.unlogged))
	}

	/// Our own store or authority failed. A line names no destination.
	fn internal(&self, kind: &'static str, connection: Option<uuid::Uuid>, cause: &str) {
		let connection = connection.map(|id| id.to_string());
		let connection = connection.as_deref();
		match self.due(kind) {
			Some(failures) => tracing::error!(
				kind,
				connection,
				failures,
				cause,
				"Egress host connection failed"
			),
			None => tracing::debug!(kind, connection, cause, "Egress host connection failed"),
		}
	}
}

/// How one connection reports its failures. Under "allow all hosts" a Pod chooses
/// the host freely, so it stays out of the log; the connection's record has it.
#[derive(Clone, Default)]
struct FailureReport {
	failures: Arc<FailureLog>,
	connection: uuid::Uuid,
	host: String,
}
impl FailureReport {
	fn cause(&self, cause: impl std::fmt::Display) -> String {
		let cause = cause.to_string();
		if self.host.is_empty() {
			return cause;
		}
		cause.replace(&self.host, "[host]")
	}

	fn upstream(&self, kind: &'static str, cause: impl std::fmt::Display) {
		metrics::counter!("aidash_egress_host_connection_failures_total", "kind" => kind)
			.increment(1);
		let cause = self.cause(cause);
		let connection = self.connection.to_string();
		match self.failures.due(kind) {
			Some(failures) => tracing::warn!(
				kind,
				connection,
				failures,
				cause,
				"Egress host connection failed"
			),
			None => tracing::debug!(kind, connection, cause, "Egress host connection failed"),
		}
	}

	fn internal(&self, kind: &'static str, cause: impl std::fmt::Display) {
		self.failures
			.internal(kind, Some(self.connection), &self.cause(cause));
	}
}

#[derive(Default)]
struct CapacityCounts {
	scopes: std::collections::HashMap<aidash_domain::egress::ScopeId, usize>,
	tenants: std::collections::HashMap<String, usize>,
}
type ScopeCounts = Arc<std::sync::Mutex<CapacityCounts>>;

/// Scope tunnel capacity is separate from the HTTP listener's request pool.
pub(super) struct TunnelManager {
	global: Arc<tokio::sync::Semaphore>,
	tenant_limit: usize,
	counts: ScopeCounts,
	tasks: std::sync::Mutex<tokio::task::JoinSet<()>>,
	stopping: tokio::sync::watch::Receiver<bool>,
	failures: Arc<FailureLog>,
}
struct TunnelLease {
	permit: Arc<TunnelPermit>,
	manager: Arc<TunnelManager>,
}
pub(super) struct TunnelPermit {
	_global: tokio::sync::OwnedSemaphorePermit,
	counts: ScopeCounts,
	scope: aidash_domain::egress::ScopeId,
}
impl Drop for TunnelPermit {
	fn drop(&mut self) {
		let mut counts = self.counts.lock().unwrap();
		let n = counts.scopes.get_mut(&self.scope).unwrap();
		*n -= 1;
		if *n == 0 {
			counts.scopes.remove(&self.scope);
		}
		let n = counts.tenants.get_mut(&self.scope.tenant).unwrap();
		*n -= 1;
		if *n == 0 {
			counts.tenants.remove(&self.scope.tenant);
		}
	}
}
impl TunnelManager {
	pub(super) fn new(
		stopping: tokio::sync::watch::Receiver<bool>,
		limits: super::ProxyLimits,
	) -> Self {
		Self {
			global: Arc::new(tokio::sync::Semaphore::new(limits.global_concurrent)),
			tenant_limit: limits.tenant_concurrent,
			counts: Arc::new(std::sync::Mutex::new(CapacityCounts::default())),
			tasks: std::sync::Mutex::new(tokio::task::JoinSet::new()),
			stopping,
			failures: Arc::default(),
		}
	}
	fn acquire(&self, scope: &ScopeSettings) -> Result<TunnelPermit, &'static str> {
		let mut counts = self.counts.lock().unwrap();
		if counts.scopes.get(&scope.id).copied().unwrap_or(0) >= scope.tunnel_limits.concurrent {
			return Err("egress_tunnel_limit");
		}
		if counts.tenants.get(&scope.id.tenant).copied().unwrap_or(0) >= self.tenant_limit {
			return Err("egress_tenant_tunnel_limit");
		}
		let global = self
			.global
			.clone()
			.try_acquire_owned()
			.map_err(|_| "egress_global_tunnel_limit")?;
		*counts.scopes.entry(scope.id.clone()).or_default() += 1;
		*counts.tenants.entry(scope.id.tenant.clone()).or_default() += 1;
		Ok(TunnelPermit {
			_global: global,
			counts: self.counts.clone(),
			scope: scope.id.clone(),
		})
	}
	pub(super) async fn drain(&self) {
		loop {
			let mut tasks = std::mem::take(&mut *self.tasks.lock().unwrap());
			if tasks.is_empty() {
				break;
			}
			while tasks.join_next().await.is_some() {}
		}
	}
}

/// Reserve before forwarding. A crash can leave unused reservation charged,
/// but never permits traffic beyond the durable daily budget.
struct Batch {
	scopes: Arc<dyn EgressScopes>,
	settings: ScopeSettings,
	direction: Direction,
	day: chrono::NaiveDate,
	remaining: u64,
	failure: FailureReport,
}
/// A failed debit, named so that `tunnel_ended` tells our own store from the
/// upstream. A decision keeps the code it was made with.
fn debit_failed(error: aidash_application::Error) -> io::Error {
	use aidash_application::Error;
	match error {
		Error::Refused(code) => io::Error::other(code),
		Error::Forbidden | Error::Unauthorized => io::Error::other("egress_credential_revoked"),
		error => io::Error::other(format!("egress_authority_unavailable: {error}")),
	}
}
impl Batch {
	fn new(
		scopes: Arc<dyn EgressScopes>,
		settings: ScopeSettings,
		direction: Direction,
		failure: FailureReport,
	) -> Self {
		Self {
			scopes,
			settings,
			direction,
			day: chrono::Utc::now().date_naive(),
			remaining: 0,
			failure,
		}
	}
	async fn charge(&mut self, bytes: usize) -> io::Result<()> {
		let day = chrono::Utc::now().date_naive();
		if day != self.day {
			if self.remaining != 0 {
				// Reported like the refund that ends a tunnel, which this one is.
				if let Err(error) = self
					.scopes
					.refund(&self.settings, self.direction, self.remaining, self.day)
					.await
				{
					let error = io::Error::other(format!("egress_refund_failed: {error}"));
					super::settle_failure("refund", "abandoned");
					self.failure.internal("refund", &error);
					return Err(error);
				}
			}
			self.day = day;
			self.remaining = 0;
		}
		let bytes = bytes as u64;
		if self.remaining < bytes {
			let need = bytes - self.remaining;
			let amount = BATCH.max(need);
			match self
				.scopes
				.debit(&self.settings, self.direction, amount, day)
				.await
			{
				Ok(()) => self.remaining += amount,
				Err(aidash_application::Error::Refused(code))
					if code == "egress_budget_exhausted" && amount != need =>
				{
					self.scopes
						.debit(&self.settings, self.direction, need, day)
						.await
						.map_err(debit_failed)?;
					self.remaining += need;
				}
				Err(error) => return Err(debit_failed(error)),
			}
		}
		Ok(())
	}
}
impl Batch {
	async fn settle(&mut self) -> io::Result<()> {
		let remaining = std::mem::take(&mut self.remaining);
		if remaining != 0 {
			self.scopes
				.refund(&self.settings, self.direction, remaining, self.day)
				.await
				.map_err(|error| io::Error::other(format!("egress_refund_failed: {error}")))?;
		}
		Ok(())
	}
}

async fn pump<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
	mut read: R,
	mut write: W,
	batch: &mut Batch,
	activity: tokio::sync::watch::Sender<()>,
	mut cancel: tokio::sync::watch::Receiver<bool>,
	count: Arc<AtomicU64>,
) -> io::Result<()> {
	let mut buffer = vec![0u8; BATCH as usize];
	loop {
		if *cancel.borrow() {
			return Err(io::Error::other("egress_cancelled"));
		}
		let n = tokio::select! {
			n = read.read(&mut buffer) => n?,
			_ = cancel.changed() => return Err(io::Error::other("egress_cancelled")),
		};
		if n == 0 {
			return tokio::select! {
				result = write.shutdown() => result,
				_ = cancel.changed() => Err(io::Error::other("egress_cancelled")),
			};
		}
		activity.send_replace(());
		tokio::select! {
			result = tokio::time::timeout(std::time::Duration::from_secs(5), batch.charge(n)) => result.map_err(|_| io::Error::other("egress_authority_unavailable"))??,
			_ = cancel.changed() => return Err(io::Error::other("egress_cancelled")),
		}
		let mut written = 0;
		while written < n {
			if *cancel.borrow() {
				return Err(io::Error::other("egress_cancelled"));
			}
			let result = tokio::select! {
				result = write.write(&buffer[written..n]) => result,
				_ = cancel.changed() => return Err(io::Error::other("egress_cancelled")),
			};
			match result {
				Ok(0) => {
					return Err(io::Error::new(io::ErrorKind::WriteZero, "upstream closed"));
				}
				Ok(bytes) => {
					activity.send_replace(());
					written += bytes;
					batch.remaining -= bytes as u64;
					count.fetch_add(bytes as u64, Ordering::Relaxed);
				}
				// Only a failed read keeps its kind, so that a tunnel can tell which
				// side went away.
				Err(error) => {
					return Err(io::Error::other(format!("{WRITE_FAILED}: {error}")));
				}
			}
		}
	}
}

#[cfg(test)]
async fn tunnel<C, U>(
	client: C,
	upstream: U,
	scopes: Arc<dyn EgressScopes>,
	settings: ScopeSettings,
) -> (u64, u64, io::Result<()>)
where
	C: AsyncRead + AsyncWrite + Unpin,
	U: AsyncRead + AsyncWrite + Unpin,
{
	let (_stop, receiver) = tokio::sync::watch::channel(false);
	tunnel_controlled(
		client,
		upstream,
		scopes,
		settings,
		"token".into(),
		"registry.npmjs.org".into(),
		TunnelReports::default(),
		receiver,
	)
	.await
}

#[derive(Default)]
struct TunnelReports {
	sent: Arc<AtomicU64>,
	received: Arc<AtomicU64>,
	failure: FailureReport,
}

#[allow(clippy::too_many_arguments)] // The tunnel binds both streams, authority and lifecycle state.
async fn tunnel_controlled<C, U>(
	client: C,
	upstream: U,
	scopes: Arc<dyn EgressScopes>,
	settings: ScopeSettings,
	credential: String,
	host: String,
	counts: TunnelReports,
	mut stopping: tokio::sync::watch::Receiver<bool>,
) -> (u64, u64, io::Result<()>)
where
	C: AsyncRead + AsyncWrite + Unpin,
	U: AsyncRead + AsyncWrite + Unpin,
{
	let (client_read, client_write) = tokio::io::split(client);
	let (upstream_read, upstream_write) = tokio::io::split(upstream);
	let sent = counts.sent;
	let received = counts.received;
	let failure = counts.failure;
	let mut sent_batch = Batch::new(
		scopes.clone(),
		settings.clone(),
		Direction::Sent,
		failure.clone(),
	);
	let mut received_batch = Batch::new(
		scopes.clone(),
		settings.clone(),
		Direction::Received,
		failure.clone(),
	);
	let (activity_tx, mut activity_rx) = tokio::sync::watch::channel(());
	let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
	let result = {
		let transfer = async {
			let send = async {
				let result = pump(
					client_read,
					upstream_write,
					&mut sent_batch,
					activity_tx.clone(),
					cancel_rx.clone(),
					sent.clone(),
				)
				.await;
				if result.is_err() {
					cancel_tx.send_replace(true);
				}
				result.map_err(|error| match error.kind() {
					io::ErrorKind::ConnectionReset => io::Error::other(CLIENT_CLOSED),
					_ => error,
				})
			};
			let receive = async {
				let result = pump(
					upstream_read,
					client_write,
					&mut received_batch,
					activity_tx.clone(),
					cancel_rx.clone(),
					received.clone(),
				)
				.await;
				if result.is_err() {
					cancel_tx.send_replace(true);
				}
				result.map_err(|error| {
					if error.to_string().starts_with(WRITE_FAILED) {
						io::Error::other(CLIENT_CLOSED)
					} else {
						error
					}
				})
			};
			let (sent, received) = tokio::join!(send, receive);
			match (sent, received) {
				(Err(first), Err(second)) if first.to_string() == "egress_cancelled" => Err(second),
				(Err(first), _) => Err(first),
				(_, result) => result,
			}
		};
		tokio::pin!(transfer);
		let lifetime = tokio::time::sleep(settings.tunnel_limits.max_lifetime);
		tokio::pin!(lifetime);
		let idle = tokio::time::sleep(settings.tunnel_limits.idle_timeout);
		tokio::pin!(idle);
		let monitor = async {
			let mut checks = tokio::time::interval(std::time::Duration::from_secs(5));
			checks.tick().await;
			let mut failures = 0;
			loop {
				checks.tick().await;
				let cause = match tokio::time::timeout(
					std::time::Duration::from_secs(5),
					scopes.check_active(&settings, &credential),
				)
				.await
				{
					Ok(Ok(current)) => {
						failures = 0;
						if !current
							.allowed_hosts
							.permits(&host, current.operator_allows_all)
						{
							break io::Error::other("egress_host_revoked");
						}
						continue;
					}
					Ok(Err(aidash_application::Error::Refused(code))) => {
						break io::Error::other(code);
					}
					Ok(Err(
						aidash_application::Error::Forbidden
						| aidash_application::Error::Unauthorized,
					)) => break io::Error::other("egress_credential_revoked"),
					Ok(Err(error)) => error.to_string().replace(&credential, "[token]"),
					Err(_) => "the check timed out".to_owned(),
				};
				failures += 1;
				if failures >= 3 {
					break io::Error::other(format!("egress_authority_unavailable: {cause}"));
				}
			}
		};
		tokio::pin!(monitor);
		let stopped = loop {
			if *stopping.borrow() {
				break (Some(io::Error::other("egress_shutdown")), None);
			}
			tokio::select! {
				result = &mut transfer => break (None, Some(result)),
				_ = &mut idle => break (Some(io::Error::new(io::ErrorKind::TimedOut, "egress_idle_timeout")), None),
				_ = activity_rx.changed() => idle.as_mut().reset(tokio::time::Instant::now() + settings.tunnel_limits.idle_timeout),
				_ = &mut lifetime => break (Some(io::Error::new(io::ErrorKind::TimedOut, "egress_lifetime_exceeded")), None),
				changed = stopping.changed() => if changed.is_err() || *stopping.borrow() { break (Some(io::Error::other("egress_shutdown")), None); },
				error = &mut monitor => break (Some(error), None),
			}
		};
		cancel_tx.send_replace(true);
		match stopped {
			(Some(error), _) => {
				let _ = transfer.await;
				Err(error)
			}
			(None, Some(result)) => result,
			(None, None) => unreachable!(),
		}
	};
	let sent_settle = tokio::time::timeout(std::time::Duration::from_secs(5), sent_batch.settle())
		.await
		.unwrap_or_else(|_| Err(io::Error::other("egress_refund_timeout")));
	let received_settle =
		tokio::time::timeout(std::time::Duration::from_secs(5), received_batch.settle())
			.await
			.unwrap_or_else(|_| Err(io::Error::other("egress_refund_timeout")));
	for settled in [&sent_settle, &received_settle] {
		if let Err(error) = settled {
			super::settle_failure("refund", "abandoned");
			failure.internal("refund", error);
		}
	}
	(
		sent.load(Ordering::Relaxed),
		received.load(Ordering::Relaxed),
		result.and(sent_settle).and(received_settle),
	)
}

/// Resolve once and connect to the verified addresses, preventing DNS rebinding.
async fn public_addresses(host: &str, port: u16) -> io::Result<Vec<std::net::SocketAddr>> {
	let lookup = host
		.strip_prefix('[')
		.and_then(|value| value.strip_suffix(']'))
		.unwrap_or(host);
	let addresses: Vec<_> =
		tokio::time::timeout(super::IDLE, tokio::net::lookup_host((lookup, port)))
			.await
			.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "egress_dns_timeout"))??
			.collect();
	if addresses.is_empty()
		|| addresses
			.iter()
			.any(|address| !aidash_domain::capabilities::outbound::public_ip(address.ip()))
	{
		return Err(io::Error::new(
			io::ErrorKind::PermissionDenied,
			"destination_forbidden",
		));
	}
	Ok(addresses)
}

struct Audit {
	id: uuid::Uuid,
	manager: Arc<TunnelManager>,
	scopes: Arc<dyn EgressScopes>,
	scope: aidash_domain::egress::ScopeId,
	host: String,
	port: u16,
	started: std::time::Instant,
	sent: Arc<AtomicU64>,
	received: Arc<AtomicU64>,
	outcome: &'static str,
	finished: bool,
	started_record: bool,
}
/// The start record holds up the connection, so it gives up quickly. During a
/// stall its cause reads as a timeout; the database's own error shows at
/// admission, which runs first and waits out the pool's 10 s.
async fn write_start_record(
	scopes: &dyn EgressScopes,
	record: aidash_application::egress::ConnectionRecord,
) -> Result<(), String> {
	let mut cause = String::new();
	for attempt in 0..3 {
		match tokio::time::timeout(
			std::time::Duration::from_secs(2),
			scopes.record(record.clone()),
		)
		.await
		{
			Ok(Ok(())) => return Ok(()),
			Ok(Err(error)) => cause = error.to_string(),
			Err(_) => cause = "attempt timed out".to_owned(),
		}
		if attempt < 2 {
			tokio::time::sleep(std::time::Duration::from_millis(100)).await;
		}
	}
	Err(cause)
}

/// Nobody waits on the final record, so it outlasts a stall of about a minute,
/// unless the process is stopping: then it gets one more attempt and no more.
async fn settle_record(
	scopes: Arc<dyn EgressScopes>,
	record: aidash_application::egress::ConnectionRecord,
	mut stopping: tokio::sync::watch::Receiver<bool>,
	report: FailureReport,
) {
	let mut settling = super::Settling::new();
	loop {
		let limit = if is_stopping(&stopping) {
			STOPPING_ATTEMPT
		} else {
			super::SETTLE_ATTEMPT
		};
		let cause = match tokio::time::timeout(limit, scopes.record(record.clone())).await {
			Ok(Ok(())) => return,
			Ok(Err(error)) => error.to_string(),
			Err(_) => "attempt timed out".to_owned(),
		};
		let retrying = !is_stopping(&stopping)
			&& tokio::select! {
				waited = settling.wait() => waited,
				_ = stopping.changed() => true,
			};
		if !retrying {
			super::settle_failure("connection_record", "abandoned");
			report.internal("final_record", cause);
			return;
		}
		super::settle_failure("connection_record", "retried");
		tracing::debug!(
			connection = %record.id,
			cause = %report.cause(cause),
			"Egress connection record failed; retrying"
		);
	}
}

fn is_stopping(stopping: &tokio::sync::watch::Receiver<bool>) -> bool {
	*stopping.borrow() || stopping.has_changed().is_err()
}

impl Audit {
	fn record(&self) -> aidash_application::egress::ConnectionRecord {
		aidash_application::egress::ConnectionRecord {
			id: self.id,
			scope: self.scope.clone(),
			host: self.host.clone(),
			port: self.port,
			sent_bytes: self.sent.load(Ordering::Relaxed),
			received_bytes: self.received.load(Ordering::Relaxed),
			duration_ms: self.started.elapsed().as_millis().min(u64::MAX as u128) as u64,
			outcome: self.outcome.to_owned(),
		}
	}
	fn report(&self) -> FailureReport {
		FailureReport {
			failures: self.manager.failures.clone(),
			connection: self.id,
			host: self.host.clone(),
		}
	}
	async fn start(&mut self) -> bool {
		if self.started_record {
			return true;
		}
		let mut record = self.record();
		if self.outcome != INVALID_TARGET {
			record.outcome = UNSETTLED.into();
		}
		match write_start_record(&*self.scopes, record).await {
			Ok(()) => self.started_record = true,
			Err(cause) => {
				self.report().internal("start_record", cause);
				self.finished = true;
			}
		}
		self.started_record
	}
	async fn finish(&mut self) {
		settle_record(
			self.scopes.clone(),
			self.record(),
			self.manager.stopping.clone(),
			self.report(),
		)
		.await;
		self.finished = true;
	}
}
impl Drop for Audit {
	fn drop(&mut self) {
		if self.finished {
			return;
		}
		let settling = settle_record(
			self.scopes.clone(),
			self.record(),
			self.manager.stopping.clone(),
			self.report(),
		);
		let mut tasks = self.manager.tasks.lock().unwrap();
		while tasks.try_join_next().is_some() {}
		tasks.spawn(settling);
	}
}

fn credential(headers: &hyper::HeaderMap) -> Option<String> {
	use base64::Engine;
	let mut values = headers.get_all(hyper::header::PROXY_AUTHORIZATION).iter();
	let value = values.next()?.to_str().ok()?;
	if values.next().is_some() || value.len() > 4096 {
		return None;
	}
	let (scheme, value) = value.split_once(' ')?;
	if scheme.eq_ignore_ascii_case("bearer") && !value.is_empty() {
		return Some(value.to_owned());
	}
	if scheme.eq_ignore_ascii_case("basic") {
		let decoded = base64::engine::general_purpose::STANDARD
			.decode(value)
			.ok()?;
		let decoded = std::str::from_utf8(&decoded).ok()?;
		let (_, token) = decoded.split_once(':')?;
		if !token.is_empty() {
			return Some(token.to_owned());
		}
	}
	None
}

fn auth_required() -> hyper::Response<super::Body> {
	let mut response = super::refuse(
		hyper::StatusCode::PROXY_AUTHENTICATION_REQUIRED,
		"proxy_auth_required",
	);
	response.headers_mut().insert(
		hyper::header::PROXY_AUTHENTICATE,
		hyper::header::HeaderValue::from_static("Basic realm=\"aidash-egress\""),
	);
	response
}

/// The Sandbox sees only the code; an unexpected cause stays in the server's log.
/// Where its text came from is not under this function's control, so the token
/// is taken back out.
fn admission_refusal(
	error: &aidash_application::Error,
	token: &str,
	failures: &FailureLog,
) -> (hyper::StatusCode, &'static str) {
	let refused = refusal(error);
	if refused.0 == hyper::StatusCode::SERVICE_UNAVAILABLE {
		let cause = error.to_string().replace(token, "[token]");
		failures.internal("admission", None, &cause);
	}
	refused
}

fn refusal(error: &aidash_application::Error) -> (hyper::StatusCode, &'static str) {
	use aidash_application::Error;
	match error {
		Error::Unauthorized => (
			hyper::StatusCode::PROXY_AUTHENTICATION_REQUIRED,
			"proxy_auth_required",
		),
		Error::Forbidden => (hyper::StatusCode::FORBIDDEN, "egress_forbidden"),
		Error::Refused(code) if code == "egress_budget_exhausted" => (
			hyper::StatusCode::TOO_MANY_REQUESTS,
			"egress_budget_exhausted",
		),
		Error::Refused(code) if code == "egress_suspended" => {
			(hyper::StatusCode::FORBIDDEN, "egress_suspended")
		}
		_ => (hyper::StatusCode::SERVICE_UNAVAILABLE, "egress_unavailable"),
	}
}

pub(super) async fn forward(
	request: hyper::Request<hyper::body::Incoming>,
	scopes: Arc<dyn EgressScopes>,
	manager: Arc<TunnelManager>,
	listener_permit: Arc<std::sync::Mutex<Option<tokio::sync::OwnedSemaphorePermit>>>,
	connection_lease: Arc<std::sync::Mutex<Option<Arc<TunnelPermit>>>>,
) -> hyper::Response<super::Body> {
	use hyper::{Method, StatusCode};
	let Some(token) = credential(request.headers()) else {
		return auth_required();
	};
	let scope = match scopes.resolve(&token).await {
		Ok(scope) => scope,
		Err(error) => {
			let (status, code) = admission_refusal(&error, &token, &manager.failures);
			if status == StatusCode::PROXY_AUTHENTICATION_REQUIRED {
				return auth_required();
			}
			return super::refuse(status, code);
		}
	};
	if let Err(error) = scopes.admit_connection(&scope).await {
		let (status, code) = admission_refusal(&error, &token, &manager.failures);
		return super::refuse(status, code);
	}
	let connect = request.method() == Method::CONNECT;
	let parsed = if connect {
		request
			.uri()
			.authority()
			.and_then(|a| url::Url::parse(&format!("https://{a}/")).ok())
	} else {
		url::Url::parse(&request.uri().to_string()).ok()
	};
	let Some(url) = parsed else {
		return super::refuse(StatusCode::BAD_REQUEST, "invalid_proxy_target");
	};
	let Some(host) = url.host_str() else {
		return super::refuse(StatusCode::BAD_REQUEST, "invalid_proxy_target");
	};
	if host.len() > 253 {
		return super::refuse(StatusCode::BAD_REQUEST, "invalid_proxy_target");
	}
	let host = host.to_owned();
	let Some(port) = url.port_or_known_default() else {
		return super::refuse(StatusCode::BAD_REQUEST, "invalid_proxy_target");
	};
	let mut audit = Audit {
		id: uuid::Uuid::new_v4(),
		manager: manager.clone(),
		scopes: scopes.clone(),
		scope: scope.id.clone(),
		host: host.clone(),
		port,
		started: std::time::Instant::now(),
		sent: Arc::new(AtomicU64::new(0)),
		received: Arc::new(AtomicU64::new(0)),
		outcome: "refused",
		finished: false,
		started_record: false,
	};
	let valid_target = url.username().is_empty()
		&& url.password().is_none()
		&& url.fragment().is_none()
		&& ((connect && url.scheme() == "https" && port == 443)
			|| (!connect && url.scheme() == "http" && port == 80));
	if !valid_target {
		audit.outcome = INVALID_TARGET;
	}
	if !audit.start().await {
		return super::refuse(StatusCode::SERVICE_UNAVAILABLE, "egress_audit_unavailable");
	}
	if !valid_target {
		return super::failure(StatusCode::BAD_REQUEST, INVALID_TARGET);
	}

	if !scope
		.allowed_hosts
		.permits(&host, scope.operator_allows_all)
	{
		return super::failure(StatusCode::FORBIDDEN, "egress_host_forbidden");
	}
	let permit = match manager.acquire(&scope) {
		Ok(permit) => Arc::new(permit),
		Err(code) => return super::failure(StatusCode::TOO_MANY_REQUESTS, code),
	};
	// A plain HTTP connection remains charged until Hyper finishes flushing it.
	if connect {
		drop(listener_permit.lock().unwrap().take());
	} else {
		*connection_lease.lock().unwrap() = Some(permit.clone());
	}
	let addresses = match public_addresses(&host, port).await {
		Ok(addresses) => addresses,
		Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
			return super::failure(StatusCode::FORBIDDEN, "egress_destination_forbidden");
		}
		Err(error) => {
			audit.report().upstream("dns", error);
			audit.outcome = "dns_error";
			return super::failure(StatusCode::BAD_GATEWAY, "egress_dns_failed");
		}
	};
	if connect {
		let upstream = match connect_upstream(&addresses).await {
			Ok(upstream) => upstream,
			Err(error) => {
				audit.report().upstream("connect", error);
				audit.outcome = "connect_error";
				return super::failure(StatusCode::BAD_GATEWAY, "egress_connect_failed");
			}
		};
		return connect_tunnel(
			request, upstream, scopes, scope, token, audit, permit, manager,
		)
		.await;
	}
	forward_http(
		request,
		url,
		addresses,
		scopes,
		scope,
		token,
		audit,
		TunnelLease { permit, manager },
	)
	.await
}

async fn connect_upstream(addresses: &[std::net::SocketAddr]) -> io::Result<tokio::net::TcpStream> {
	tokio::time::timeout(super::IDLE, tokio::net::TcpStream::connect(addresses))
		.await
		.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "connecting timed out"))?
}

/// The outcome for a connection's record. An end with no named code leaves its
/// cause in the log, as does a lost authority, whose code alone does not say why.
fn tunnel_ended(report: &FailureReport, result: &io::Result<()>) -> &'static str {
	let outcome = tunnel_outcome(result);
	if let Err(error) = result {
		match outcome {
			"interrupted" => report.upstream("transfer", error),
			"egress_authority_unavailable" => report.internal("authority", error),
			_ => {}
		}
	}
	outcome
}

fn tunnel_outcome(result: &io::Result<()>) -> &'static str {
	let Err(error) = result else {
		return "completed";
	};
	let message = error.to_string();
	for code in [
		"egress_budget_exhausted",
		"egress_suspended",
		"egress_idle_timeout",
		"egress_lifetime_exceeded",
		"egress_shutdown",
		"egress_credential_revoked",
		"egress_host_revoked",
		"egress_authority_unavailable",
		"egress_refund_failed",
		"egress_refund_timeout",
		CLIENT_CLOSED,
	] {
		if message.contains(code) {
			return code;
		}
	}
	"interrupted"
}

#[allow(clippy::too_many_arguments)] // The upgrade carries its resolved scope, audit and lease.
async fn connect_tunnel(
	mut request: hyper::Request<hyper::body::Incoming>,
	upstream: tokio::net::TcpStream,
	scopes: Arc<dyn EgressScopes>,
	scope: ScopeSettings,
	credential: String,
	mut audit: Audit,
	permit: Arc<TunnelPermit>,
	manager: Arc<TunnelManager>,
) -> hyper::Response<super::Body> {
	audit.outcome = "interrupted";
	if !audit.start().await {
		return super::failure(
			hyper::StatusCode::SERVICE_UNAVAILABLE,
			"egress_audit_unavailable",
		);
	}
	let report = audit.report();
	let sent = audit.sent.clone();
	let received = audit.received.clone();
	let upgrade = hyper::upgrade::on(&mut request);
	let stopping = manager.stopping.clone();
	let mut tasks = manager.tasks.lock().unwrap();
	while tasks.try_join_next().is_some() {}
	tasks.spawn(async move {
		match tokio::time::timeout(std::time::Duration::from_secs(5), upgrade).await {
			Ok(Ok(client)) => {
				let (_, _, result) = tunnel_controlled(
					hyper_util::rt::TokioIo::new(client),
					upstream,
					scopes,
					scope,
					credential,
					audit.host.clone(),
					TunnelReports {
						sent: sent.clone(),
						received: received.clone(),
						failure: report.clone(),
					},
					stopping,
				)
				.await;
				audit.outcome = tunnel_ended(&report, &result);
			}
			Ok(Err(error)) => {
				report.upstream("upgrade", super::causes(&error));
				audit.outcome = "upgrade_failed";
			}
			Err(_) => {
				report.upstream("upgrade", "the upgrade timed out");
				audit.outcome = "upgrade_failed";
			}
		}
		// A stall must not hold the Scratch Space's limit while the record retries.
		drop(permit);
		audit.finish().await;
	});
	hyper::Response::builder()
		.status(hyper::StatusCode::OK)
		.body(
			http_body_util::Full::new(bytes::Bytes::new())
				.map_err(|never| match never {})
				.boxed_unsync(),
		)
		.unwrap()
}

#[allow(clippy::too_many_arguments)] // The forwarder carries the admitted target, audit and lease.
async fn forward_http(
	request: hyper::Request<hyper::body::Incoming>,
	url: url::Url,
	addresses: Vec<std::net::SocketAddr>,
	scopes: Arc<dyn EgressScopes>,
	scope: ScopeSettings,
	credential: String,
	mut audit: Audit,
	lease: TunnelLease,
) -> hyper::Response<super::Body> {
	let TunnelLease { permit, manager } = lease;
	use hyper::{StatusCode, header};
	if !audit.start().await {
		return super::failure(StatusCode::SERVICE_UNAVAILABLE, "egress_audit_unavailable");
	}
	let upstream = match connect_upstream(&addresses).await {
		Ok(socket) => socket,
		Err(error) => {
			audit.report().upstream("connect", error);
			audit.outcome = "connect_error";
			return super::failure(StatusCode::BAD_GATEWAY, "egress_connect_failed");
		}
	};
	audit.outcome = "interrupted";
	let report = audit.report();
	let (client, proxy) = tokio::io::duplex(64 * 1024);
	let sent = audit.sent.clone();
	let received = audit.received.clone();
	let (result_tx, mut transfer) = tokio::sync::oneshot::channel();
	let stopping = manager.stopping.clone();
	{
		let mut tasks = manager.tasks.lock().unwrap();
		while tasks.try_join_next().is_some() {}
		let report = report.clone();
		tasks.spawn(async move {
			let (_, _, result) = tunnel_controlled(
				proxy,
				upstream,
				scopes,
				scope,
				credential,
				audit.host.clone(),
				TunnelReports {
					sent: sent.clone(),
					received: received.clone(),
					failure: report.clone(),
				},
				stopping,
			)
			.await;
			let outcome = tunnel_ended(&report, &result);
			audit.outcome = outcome;
			let _ = result_tx.send(outcome);
			// A stall must not hold the Scratch Space's limit while the record retries.
			drop(permit);
			audit.finish().await;
		});
	}
	let (mut sender, connection) =
		match hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(client)).await {
			Ok(pair) => pair,
			Err(error) => {
				report.upstream("handshake", super::causes(&error));
				return super::failure(StatusCode::BAD_GATEWAY, "egress_connect_failed");
			}
		};
	tokio::spawn(async move {
		let _ = connection.await;
	});
	let (mut parts, body) = request.into_parts();
	let target = match url[url::Position::BeforePath..].parse() {
		Ok(target) => target,
		Err(_) => return super::failure(StatusCode::BAD_REQUEST, "invalid_proxy_target"),
	};
	parts.uri = target;
	let connection_headers = parts
		.headers
		.get("connection")
		.and_then(|v| v.to_str().ok())
		.unwrap_or("")
		.split(',')
		.map(|s| s.trim().to_ascii_lowercase())
		.collect::<Vec<_>>();
	for name in parts.headers.keys().cloned().collect::<Vec<_>>() {
		if matches!(
			name.as_str(),
			"proxy-authorization"
				| "proxy-authenticate"
				| "connection"
				| "keep-alive"
				| "transfer-encoding"
				| "upgrade" | "te"
				| "trailer" | "host"
		) || connection_headers.iter().any(|item| item == name.as_str())
		{
			parts.headers.remove(name);
		}
	}
	let host = &url[url::Position::BeforeHost..url::Position::AfterPort];
	let Ok(host) = header::HeaderValue::from_str(host) else {
		return super::failure(StatusCode::BAD_REQUEST, "invalid_proxy_target");
	};
	parts.headers.insert(header::HOST, host);
	parts.headers.insert(
		header::CONNECTION,
		header::HeaderValue::from_static("close"),
	);
	let pending_response = sender.send_request(hyper::Request::from_parts(parts, body));
	tokio::pin!(pending_response);
	let mut transfer_completed = false;
	let response = tokio::select! {
		response = &mut pending_response => response,
		outcome = &mut transfer => match outcome {
			// The transfer may finish after writing the response into the duplex
			// buffer but before Hyper has parsed its headers.
			Ok("completed") => { transfer_completed = true; pending_response.await },
			Ok("egress_budget_exhausted") => return super::failure(StatusCode::TOO_MANY_REQUESTS, "egress_budget_exhausted"),
			Ok("egress_suspended") => return super::failure(StatusCode::FORBIDDEN, "egress_suspended"),
			ended => {
				let code = ended.unwrap_or("the transfer stopped");
				report.upstream("request", format!("ended before a response: {code}"));
				return super::failure(StatusCode::BAD_GATEWAY, "egress_upstream_failed");
			}
		},
	};
	let response = match response {
		Ok(response) => response,
		Err(error) => {
			report.upstream("request", super::causes(&error));
			return super::failure(StatusCode::BAD_GATEWAY, "egress_upstream_failed");
		}
	};
	let (mut parts, body) = response.into_parts();
	let close_delimited = !parts.headers.contains_key(header::CONTENT_LENGTH)
		&& !parts.headers.contains_key(header::TRANSFER_ENCODING)
		&& !body.is_end_stream();
	let connection_headers = parts
		.headers
		.get("connection")
		.and_then(|v| v.to_str().ok())
		.unwrap_or("")
		.split(',')
		.map(|s| s.trim().to_ascii_lowercase())
		.collect::<Vec<_>>();
	for name in parts.headers.keys().cloned().collect::<Vec<_>>() {
		if matches!(
			name.as_str(),
			"proxy-authenticate"
				| "connection"
				| "keep-alive"
				| "transfer-encoding"
				| "upgrade" | "te"
				| "trailer"
		) || connection_headers.iter().any(|item| item == name.as_str())
		{
			parts.headers.remove(name);
		}
	}
	let stream = async_stream::try_stream! {
		let mut body = body;
		while let Some(frame) = body.frame().await {
			yield frame.map_err(|error| Box::new(error) as super::BoxError)?;
		}
		if close_delimited && !transfer_completed && !matches!(tokio::time::timeout(super::IDLE, &mut transfer).await, Ok(Ok("completed"))) {
			Err(super::io_error("egress_response_interrupted"))?;
		}
	};
	hyper::Response::from_parts(
		parts,
		http_body_util::StreamBody::new(stream).boxed_unsync(),
	)
}

#[cfg(test)]
mod tests {
	use aidash_application::egress::{EgressScopes, ScopeSettings, test_support::MemoryScopes};
	use aidash_domain::egress::{AllowedHosts, AnomalyRule, DailyBudget, ScopeId};
	use std::sync::{Arc, atomic::Ordering};
	use tokio::io::{AsyncReadExt, AsyncWriteExt};

	fn test_settings() -> ScopeSettings {
		ScopeSettings {
			id: ScopeId {
				tenant: "t".into(),
				node: "n".into(),
				scratch_space: uuid::Uuid::new_v4(),
			},
			allowed_hosts: AllowedHosts::default(),
			budget: DailyBudget::default(),
			anomaly: AnomalyRule::default(),
			operator_allows_all: false,
			tunnel_limits: Default::default(),
		}
	}

	#[test]
	fn scope_tunnel_cap_is_independent_and_releases_capacity() {
		let (_stop, receiver) = tokio::sync::watch::channel(false);
		let manager = super::TunnelManager::new(receiver, super::super::ProxyLimits::default());
		let mut one = test_settings();
		one.tunnel_limits.concurrent = 0;
		assert!(manager.acquire(&one).is_err());
		assert!(manager.counts.lock().unwrap().scopes.is_empty());
		one.tunnel_limits.concurrent = 1;
		let mut two = test_settings();
		two.tunnel_limits.concurrent = 1;
		let held = manager.acquire(&one).unwrap();
		assert!(manager.acquire(&one).is_err());
		assert!(manager.acquire(&two).is_ok());
		drop(held);
		assert!(manager.acquire(&one).is_ok());
	}

	#[test]
	fn tenants_cannot_exhaust_proxy_capacity_using_multiple_scopes() {
		let (_stop, receiver) = tokio::sync::watch::channel(false);
		let manager = super::TunnelManager::new(receiver, super::super::ProxyLimits::default());
		let first = test_settings();
		let second = test_settings();
		let held: Vec<_> = (0..32).map(|_| manager.acquire(&first).unwrap()).collect();
		assert!(manager.acquire(&second).is_err());
		let mut other_tenant = test_settings();
		other_tenant.id.tenant = "other".into();
		assert!(manager.acquire(&other_tenant).is_ok());
		drop(held);
		assert!(manager.acquire(&second).is_ok());
	}

	#[test]
	fn tenant_and_global_exhaustion_have_distinct_codes() {
		let (_stop, receiver) = tokio::sync::watch::channel(false);
		let manager = super::TunnelManager::new(
			receiver,
			super::super::ProxyLimits {
				global_concurrent: 2,
				tenant_concurrent: 1,
			},
		);
		let one = test_settings();
		let mut two = test_settings();
		two.id.tenant = "two".into();
		let mut three = test_settings();
		three.id.tenant = "three".into();
		let _one = manager.acquire(&one).unwrap();
		assert_eq!(
			manager.acquire(&one).err().unwrap(),
			"egress_tenant_tunnel_limit"
		);
		let _two = manager.acquire(&two).unwrap();
		assert_eq!(
			manager.acquire(&three).err().unwrap(),
			"egress_global_tunnel_limit"
		);
	}

	#[tokio::test(start_paused = true)]
	async fn slow_partial_writes_are_activity() {
		let mut settings = test_settings();
		settings.tunnel_limits.idle_timeout = std::time::Duration::from_secs(3);
		let scopes = Arc::new(MemoryScopes::new("token".into(), settings.clone()));
		let (mut client, gateway_client) = tokio::io::duplex(32);
		let (gateway_server, mut server) = tokio::io::duplex(1);
		let (_stop, receiver) = tokio::sync::watch::channel(false);
		let task = tokio::spawn(super::tunnel_controlled(
			gateway_client,
			gateway_server,
			scopes,
			settings,
			"token".into(),
			"registry.npmjs.org".into(),
			super::TunnelReports::default(),
			receiver,
		));
		client.write_all(b"abcdef").await.unwrap();
		for byte in b"abcdef" {
			let read = server.read_u8().await.unwrap();
			assert_eq!(read, *byte);
			tokio::time::advance(std::time::Duration::from_secs(1)).await;
			tokio::task::yield_now().await;
		}
		assert!(
			!task.is_finished(),
			"active writes must keep the tunnel open"
		);
		task.abort();
	}

	struct StalledChecks(Arc<MemoryScopes>, std::sync::atomic::AtomicUsize);
	#[async_trait::async_trait]
	impl EgressScopes for StalledChecks {
		async fn resolve(&self, c: &str) -> aidash_application::Result<ScopeSettings> {
			self.0.resolve(c).await
		}
		async fn admit_external(
			&self,
			c: &str,
			s: &str,
			m: &str,
			p: &str,
			d: &str,
		) -> aidash_application::Result<aidash_application::egress::Admission> {
			self.0.admit_external(c, s, m, p, d).await
		}
		async fn admit_connection(&self, s: &ScopeSettings) -> aidash_application::Result<()> {
			self.0.admit_connection(s).await
		}
		async fn check_active(
			&self,
			_: &ScopeSettings,
			_: &str,
		) -> aidash_application::Result<ScopeSettings> {
			std::future::pending().await
		}
		async fn debit(
			&self,
			s: &ScopeSettings,
			d: aidash_application::egress::Direction,
			n: u64,
			day: chrono::NaiveDate,
		) -> aidash_application::Result<()> {
			self.0.debit(s, d, n, day).await
		}
		async fn refund(
			&self,
			s: &ScopeSettings,
			d: aidash_application::egress::Direction,
			n: u64,
			day: chrono::NaiveDate,
		) -> aidash_application::Result<()> {
			self.0.refund(s, d, n, day).await
		}
		async fn record(
			&self,
			r: aidash_application::egress::ConnectionRecord,
		) -> aidash_application::Result<()> {
			if self
				.1
				.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
					left.checked_sub(1)
				})
				.is_ok()
			{
				return Err(aidash_application::Error::External(
					"temporary audit failure".into(),
				));
			}
			self.0.record(r).await
		}
		async fn lift_suspension(&self, id: &ScopeId) -> aidash_application::Result<()> {
			self.0.lift_suspension(id).await
		}
	}
	#[tokio::test(start_paused = true)]
	async fn shutdown_interrupts_a_stalled_authority_check() {
		let settings = test_settings();
		let scopes = Arc::new(StalledChecks(
			Arc::new(MemoryScopes::new("token".into(), settings.clone())),
			std::sync::atomic::AtomicUsize::new(0),
		));
		let (_client, gateway_client) = tokio::io::duplex(64);
		let (gateway_server, _server) = tokio::io::duplex(64);
		let (stop, receiver) = tokio::sync::watch::channel(false);
		let task = tokio::spawn(super::tunnel_controlled(
			gateway_client,
			gateway_server,
			scopes,
			settings,
			"token".into(),
			"registry.npmjs.org".into(),
			super::TunnelReports::default(),
			receiver,
		));
		tokio::task::yield_now().await;
		tokio::time::advance(std::time::Duration::from_secs(5)).await;
		tokio::task::yield_now().await;
		stop.send(true).unwrap();
		let result = tokio::time::timeout(std::time::Duration::from_secs(1), task)
			.await
			.expect("shutdown must not wait for the authority")
			.unwrap();
		assert!(
			result
				.2
				.unwrap_err()
				.to_string()
				.contains("egress_shutdown")
		);
	}

	#[tokio::test(start_paused = true)]
	async fn repeated_authority_timeouts_are_not_reported_as_suspension() {
		let settings = test_settings();
		let scopes = Arc::new(StalledChecks(
			Arc::new(MemoryScopes::new("token".into(), settings.clone())),
			std::sync::atomic::AtomicUsize::new(0),
		));
		let (_client, gateway_client) = tokio::io::duplex(64);
		let (gateway_server, _server) = tokio::io::duplex(64);
		let (_stop, receiver) = tokio::sync::watch::channel(false);
		let task = tokio::spawn(super::tunnel_controlled(
			gateway_client,
			gateway_server,
			scopes,
			settings,
			"token".into(),
			"registry.npmjs.org".into(),
			super::TunnelReports::default(),
			receiver,
		));
		tokio::task::yield_now().await;
		for _ in 0..5 {
			tokio::time::advance(std::time::Duration::from_secs(5)).await;
			tokio::task::yield_now().await;
		}
		let result = tokio::time::timeout(std::time::Duration::from_secs(1), task)
			.await
			.unwrap()
			.unwrap();
		assert!(
			result
				.2
				.unwrap_err()
				.to_string()
				.contains("egress_authority_unavailable")
		);
	}

	struct RevokedCredential(
		Arc<MemoryScopes>,
		std::sync::atomic::AtomicBool,
		std::sync::atomic::AtomicBool,
	);
	#[async_trait::async_trait]
	impl EgressScopes for RevokedCredential {
		async fn resolve(&self, c: &str) -> aidash_application::Result<ScopeSettings> {
			if self.1.load(Ordering::SeqCst) {
				return Err(aidash_application::Error::Unauthorized);
			}
			let mut current = self.0.resolve(c).await?;
			if self.2.load(Ordering::SeqCst) {
				current.allowed_hosts.hosts.clear();
			}
			Ok(current)
		}
		async fn admit_external(
			&self,
			c: &str,
			s: &str,
			m: &str,
			p: &str,
			d: &str,
		) -> aidash_application::Result<aidash_application::egress::Admission> {
			self.0.admit_external(c, s, m, p, d).await
		}
		async fn admit_connection(&self, s: &ScopeSettings) -> aidash_application::Result<()> {
			self.0.admit_connection(s).await
		}
		async fn check_active(
			&self,
			s: &ScopeSettings,
			c: &str,
		) -> aidash_application::Result<ScopeSettings> {
			let current = self.resolve(c).await?;
			self.0.check_active(s, c).await?;
			Ok(current)
		}
		async fn debit(
			&self,
			s: &ScopeSettings,
			d: aidash_application::egress::Direction,
			n: u64,
			day: chrono::NaiveDate,
		) -> aidash_application::Result<()> {
			self.0.debit(s, d, n, day).await
		}
		async fn refund(
			&self,
			s: &ScopeSettings,
			d: aidash_application::egress::Direction,
			n: u64,
			day: chrono::NaiveDate,
		) -> aidash_application::Result<()> {
			self.0.refund(s, d, n, day).await
		}
		async fn record(
			&self,
			r: aidash_application::egress::ConnectionRecord,
		) -> aidash_application::Result<()> {
			self.0.record(r).await
		}
		async fn lift_suspension(&self, id: &ScopeId) -> aidash_application::Result<()> {
			self.0.lift_suspension(id).await
		}
	}
	#[tokio::test(start_paused = true)]
	async fn revoked_credential_closes_an_existing_tunnel() {
		let settings = test_settings();
		let scopes = Arc::new(RevokedCredential(
			Arc::new(MemoryScopes::new("token".into(), settings.clone())),
			std::sync::atomic::AtomicBool::new(false),
			std::sync::atomic::AtomicBool::new(false),
		));
		let (_client, gateway_client) = tokio::io::duplex(64);
		let (gateway_server, _server) = tokio::io::duplex(64);
		let (_stop, receiver) = tokio::sync::watch::channel(false);
		let task = tokio::spawn(super::tunnel_controlled(
			gateway_client,
			gateway_server,
			scopes.clone(),
			settings,
			"token".into(),
			"registry.npmjs.org".into(),
			super::TunnelReports::default(),
			receiver,
		));
		tokio::task::yield_now().await;
		scopes.1.store(true, Ordering::SeqCst);
		tokio::time::advance(std::time::Duration::from_secs(5)).await;
		let result = tokio::time::timeout(std::time::Duration::from_secs(1), task)
			.await
			.expect("revoked credential must close the tunnel")
			.unwrap();
		assert!(result.2.is_err());
	}

	#[tokio::test(start_paused = true)]
	async fn narrowing_allowlist_closes_an_existing_tunnel() {
		let settings = test_settings();
		let scopes = Arc::new(RevokedCredential(
			Arc::new(MemoryScopes::new("token".into(), settings.clone())),
			std::sync::atomic::AtomicBool::new(false),
			std::sync::atomic::AtomicBool::new(false),
		));
		let (_client, gateway_client) = tokio::io::duplex(64);
		let (gateway_server, _server) = tokio::io::duplex(64);
		let (_stop, receiver) = tokio::sync::watch::channel(false);
		let task = tokio::spawn(super::tunnel_controlled(
			gateway_client,
			gateway_server,
			scopes.clone(),
			settings,
			"token".into(),
			"registry.npmjs.org".into(),
			super::TunnelReports::default(),
			receiver,
		));
		tokio::task::yield_now().await;
		scopes.2.store(true, Ordering::SeqCst);
		tokio::time::advance(std::time::Duration::from_secs(5)).await;
		let result = tokio::time::timeout(std::time::Duration::from_secs(1), task)
			.await
			.unwrap()
			.unwrap();
		assert!(
			result
				.2
				.unwrap_err()
				.to_string()
				.contains("egress_host_revoked")
		);
	}

	#[tokio::test(start_paused = true)]
	async fn periodic_suspension_closes_idle_tunnel_and_settles_batch() {
		let settings = test_settings();
		let scopes = Arc::new(MemoryScopes::new("token".into(), settings.clone()));
		let (mut client, gateway_client) = tokio::io::duplex(64);
		let (gateway_server, mut server) = tokio::io::duplex(64);
		let (_idle_client, idle_gateway) = tokio::io::duplex(64);
		let (idle_upstream, _idle_server) = tokio::io::duplex(64);
		let (_stop, receiver) = tokio::sync::watch::channel(false);
		let idle_task = tokio::spawn(super::tunnel_controlled(
			idle_gateway,
			idle_upstream,
			scopes.clone(),
			settings.clone(),
			"token".into(),
			"registry.npmjs.org".into(),
			super::TunnelReports::default(),
			receiver.clone(),
		));
		let task = tokio::spawn(super::tunnel_controlled(
			gateway_client,
			gateway_server,
			scopes.clone(),
			settings,
			"token".into(),
			"registry.npmjs.org".into(),
			super::TunnelReports::default(),
			receiver,
		));
		client.write_all(b"a").await.unwrap();
		let mut data = [0];
		server.read_exact(&mut data).await.unwrap();
		for _ in 0..5 {
			client.write_all(b"b").await.unwrap();
			server.read_exact(&mut data).await.unwrap();
		}
		assert_eq!(scopes.checks(), 0, "no check per read");
		tokio::task::yield_now().await;
		scopes.suspend();
		tokio::time::advance(std::time::Duration::from_secs(5)).await;
		let (sent, _, result) = task.await.unwrap();
		assert_eq!(sent, 6);
		assert!(result.unwrap_err().to_string().contains("egress_suspended"));
		assert!(
			idle_task
				.await
				.unwrap()
				.2
				.unwrap_err()
				.to_string()
				.contains("egress_suspended")
		);
		assert_eq!(scopes.usage(chrono::Utc::now().date_naive()).0, 6);
	}

	#[tokio::test(start_paused = true)]
	async fn tunnel_stops_on_idle_lifetime_and_shutdown() {
		for mode in 0..3 {
			let mut settings = test_settings();
			settings.tunnel_limits.idle_timeout =
				std::time::Duration::from_secs(if mode == 0 { 2 } else { 100 });
			settings.tunnel_limits.max_lifetime =
				std::time::Duration::from_secs(if mode == 1 { 2 } else { 100 });
			let scopes = Arc::new(MemoryScopes::new("token".into(), settings.clone()));
			let (_client, gateway_client) = tokio::io::duplex(64);
			let (gateway_server, _server) = tokio::io::duplex(64);
			let (stop, receiver) = tokio::sync::watch::channel(false);
			let task = tokio::spawn(super::tunnel_controlled(
				gateway_client,
				gateway_server,
				scopes,
				settings,
				"token".into(),
				"registry.npmjs.org".into(),
				super::TunnelReports::default(),
				receiver,
			));
			tokio::task::yield_now().await;
			if mode == 2 {
				stop.send(true).unwrap();
			} else {
				tokio::time::advance(std::time::Duration::from_secs(3)).await;
			}
			let (_, _, result) = task.await.unwrap();
			let expected = [
				"egress_idle_timeout",
				"egress_lifetime_exceeded",
				"egress_shutdown",
			][mode];
			assert!(result.unwrap_err().to_string().contains(expected));
		}
	}

	#[tokio::test]
	async fn tunnel_counts_both_directions_and_enforces_budget() {
		let settings = ScopeSettings {
			id: ScopeId {
				tenant: "t".into(),
				node: "n".into(),
				scratch_space: uuid::Uuid::new_v4(),
			},
			allowed_hosts: AllowedHosts::default(),
			budget: DailyBudget {
				sent_bytes: 3,
				received_bytes: 3,
			},
			anomaly: AnomalyRule::default(),
			operator_allows_all: false,
			tunnel_limits: Default::default(),
		};
		let scopes = Arc::new(MemoryScopes::new("token".into(), settings.clone()));
		let (mut client, gateway_client) = tokio::io::duplex(32);
		let (gateway_server, mut server) = tokio::io::duplex(32);
		let task = tokio::spawn(super::tunnel(
			gateway_client,
			gateway_server,
			scopes,
			settings,
		));
		client.write_all(b"abc").await.unwrap();
		let mut data = [0; 3];
		server.read_exact(&mut data).await.unwrap();
		assert_eq!(&data, b"abc");
		server.write_all(b"xyz").await.unwrap();
		client.read_exact(&mut data).await.unwrap();
		assert_eq!(&data, b"xyz");
		client.write_all(b"q").await.unwrap();
		let result = task.await.unwrap();
		assert_eq!(result.0, 3);
		assert_eq!(result.1, 3);
		assert!(result.2.is_err());
	}
	struct DenyLegacy;
	#[async_trait::async_trait]
	impl aidash_application::egress::Authority for DenyLegacy {
		async fn admit(
			&self,
			_: &str,
			_: &str,
			_: &str,
			_: &str,
			_: &str,
		) -> aidash_application::Result<aidash_application::egress::Admission> {
			Err(aidash_application::Error::Forbidden)
		}
	}

	#[tokio::test]
	async fn public_ipv6_literal_is_resolved_without_brackets() {
		let url = url::Url::parse("https://[2606:4700:4700::1111]/").unwrap();
		let addresses = super::public_addresses(url.host_str().unwrap(), 443)
			.await
			.unwrap();
		assert_eq!(
			addresses,
			vec!["[2606:4700:4700::1111]:443".parse().unwrap()]
		);
	}

	#[tokio::test]
	async fn over_limit_is_a_typed_http_429() {
		let mut settings = test_settings();
		settings.tunnel_limits.concurrent = 0;
		let scopes = Arc::new(MemoryScopes::new("credential".into(), settings));
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = listener.local_addr().unwrap();
		let (stop, receiver) = tokio::sync::watch::channel(false);
		let server = tokio::spawn(super::super::serve_with_scopes(
			listener,
			Arc::new(DenyLegacy),
			Some(scopes),
			super::super::ProxyLimits::default(),
			receiver,
		));
		let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
		client.write_all(b"CONNECT registry.npmjs.org:443 HTTP/1.1\r\nHost: registry.npmjs.org:443\r\nProxy-Authorization: Bearer credential\r\nConnection: close\r\n\r\n").await.unwrap();
		let mut response = Vec::new();
		client.read_to_end(&mut response).await.unwrap();
		assert!(response.starts_with(b"HTTP/1.1 429"));
		assert!(String::from_utf8_lossy(&response).contains("egress_tunnel_limit"));
		stop.send(true).unwrap();
		server.await.unwrap().unwrap();
	}

	#[tokio::test]
	async fn a_lingering_upload_releases_its_tunnel_capacity() {
		let mut settings = test_settings();
		settings.allowed_hosts.hosts = vec!["127.0.0.1".into()];
		settings.tunnel_limits.concurrent = 1;
		let scopes = Arc::new(MemoryScopes::new("token".into(), settings));
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = listener.local_addr().unwrap();
		let (stop, receiver) = tokio::sync::watch::channel(false);
		let server = tokio::spawn(super::super::serve_with_scopes(
			listener,
			Arc::new(DenyLegacy),
			Some(scopes),
			super::super::ProxyLimits::default(),
			receiver,
		));
		let mut first = tokio::net::TcpStream::connect(address).await.unwrap();
		first.write_all(b"POST http://127.0.0.1/upload HTTP/1.1\r\nHost: 127.0.0.1\r\nProxy-Authorization: Bearer token\r\nContent-Length: 1048576\r\nConnection: close\r\n\r\n").await.unwrap();
		let mut answer = Vec::new();
		tokio::time::timeout(
			std::time::Duration::from_secs(3),
			first.read_to_end(&mut answer),
		)
		.await
		.unwrap()
		.unwrap();
		assert!(String::from_utf8_lossy(&answer).contains("egress_destination_forbidden"));
		let mut second = tokio::net::TcpStream::connect(address).await.unwrap();
		second.write_all(b"GET http://127.0.0.1/ HTTP/1.1\r\nHost: 127.0.0.1\r\nProxy-Authorization: Bearer token\r\nConnection: close\r\n\r\n").await.unwrap();
		let mut answer = Vec::new();
		tokio::time::timeout(
			std::time::Duration::from_secs(3),
			second.read_to_end(&mut answer),
		)
		.await
		.unwrap()
		.unwrap();
		assert!(
			String::from_utf8_lossy(&answer).contains("egress_destination_forbidden"),
			"{}",
			String::from_utf8_lossy(&answer)
		);
		drop(first);
		stop.send(true).unwrap();
		server.await.unwrap().unwrap();
	}

	#[tokio::test]
	async fn proxy_requires_scope_credential_and_rejects_private_resolution() {
		let settings = ScopeSettings {
			id: ScopeId {
				tenant: "t".into(),
				node: "n".into(),
				scratch_space: uuid::Uuid::new_v4(),
			},
			allowed_hosts: AllowedHosts {
				hosts: vec!["127.0.0.1".into()],
				allow_all: false,
			},
			budget: DailyBudget::default(),
			anomaly: AnomalyRule {
				connections_per_minute: 2,
			},
			operator_allows_all: false,
			tunnel_limits: Default::default(),
		};
		let scope_id = settings.id.clone();
		let scopes = Arc::new(MemoryScopes::new("token".into(), settings));
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = listener.local_addr().unwrap();
		let (stop, stopping) = tokio::sync::watch::channel(false);
		let gateway = tokio::spawn(super::super::serve_with_scopes(
			listener,
			Arc::new(DenyLegacy),
			Some(scopes.clone()),
			super::super::ProxyLimits::default(),
			stopping,
		));
		async fn request(address: std::net::SocketAddr, auth: &str) -> String {
			let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
			socket
				.write_all(
					format!(
						"CONNECT 127.0.0.1:443 HTTP/1.1\r\nHost: 127.0.0.1:443\r\n{auth}Connection: close\r\n\r\n"
					)
					.as_bytes(),
				)
				.await
				.unwrap();
			let mut response = String::new();
			socket.read_to_string(&mut response).await.unwrap();
			response
		}
		assert!(request(address, "").await.starts_with("HTTP/1.1 407"));
		assert!(
			request(address, "Proxy-Authorization: Bearer invalid\r\n")
				.await
				.starts_with("HTTP/1.1 407")
		);
		assert!(
			request(address, "Proxy-Authorization: Bearer token\r\n")
				.await
				.starts_with("HTTP/1.1 403")
		);
		let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
		socket.write_all(b"GET http://127.0.0.1/private?secret=value HTTP/1.1\r\nHost: 127.0.0.1\r\nProxy-Authorization: Bearer token\r\nConnection: close\r\n\r\n").await.unwrap();
		let mut response = String::new();
		socket.read_to_string(&mut response).await.unwrap();
		assert!(response.starts_with("HTTP/1.1 403"));
		let suspended = request(address, "Proxy-Authorization: Bearer token\r\n").await;
		assert!(suspended.contains("egress_suspended"));
		assert_eq!(scopes.notifications(), 1);
		scopes.lift_suspension(&scope_id).await.unwrap();
		let lifted = request(address, "Proxy-Authorization: Bearer token\r\n").await;
		assert!(lifted.contains("egress_destination_forbidden"));
		tokio::time::timeout(std::time::Duration::from_secs(3), async {
			while scopes
				.records()
				.first()
				.is_none_or(|record| record.outcome != "refused")
			{
				tokio::task::yield_now().await;
			}
		})
		.await
		.unwrap();
		let records = scopes.records();
		assert_eq!(records[0].host, "127.0.0.1");
		assert_eq!(records[0].outcome, "refused");
		stop.send(true).unwrap();
		gateway.await.unwrap().unwrap();
	}
	#[tokio::test]
	async fn forbidden_hosts_count_toward_anomaly_suspension() {
		let mut settings = test_settings();
		settings.anomaly.connections_per_minute = 2;
		let scopes = Arc::new(MemoryScopes::new("token".into(), settings));
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = listener.local_addr().unwrap();
		let (stop, receiver) = tokio::sync::watch::channel(false);
		let server = tokio::spawn(super::super::serve_with_scopes(
			listener,
			Arc::new(DenyLegacy),
			Some(scopes.clone()),
			super::super::ProxyLimits::default(),
			receiver,
		));
		for index in 0..3 {
			let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
			client.write_all(format!("CONNECT denied{index}.example:443 HTTP/1.1\r\nHost: denied{index}.example:443\r\nProxy-Authorization: Bearer token\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
			let mut response = String::new();
			client.read_to_string(&mut response).await.unwrap();
			if index == 2 {
				assert!(response.contains("egress_suspended"), "{response}");
			}
		}
		assert_eq!(scopes.notifications(), 1);
		stop.send(true).unwrap();
		server.await.unwrap().unwrap();
	}

	#[tokio::test]
	async fn oversized_host_is_rejected_without_an_audit_row() {
		let mut settings = test_settings();
		settings.anomaly.connections_per_minute = 2;
		let scopes = Arc::new(MemoryScopes::new("token".into(), settings));
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = listener.local_addr().unwrap();
		let (stop, receiver) = tokio::sync::watch::channel(false);
		let task = tokio::spawn(super::super::serve_with_scopes(
			listener,
			Arc::new(DenyLegacy),
			Some(scopes.clone()),
			super::super::ProxyLimits::default(),
			receiver,
		));
		let host = [
			"a".repeat(63),
			"b".repeat(63),
			"c".repeat(63),
			"d".repeat(63),
		]
		.join(".");
		for attempt in 0..3 {
			let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
			client.write_all(format!("CONNECT {host}:443 HTTP/1.1\r\nHost: {host}:443\r\nProxy-Authorization: Bearer token\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
			let mut response = String::new();
			client.read_to_string(&mut response).await.unwrap();
			assert!(response.contains(if attempt == 2 {
				"egress_suspended"
			} else {
				"invalid_proxy_target"
			}));
		}

		stop.send(true).unwrap();
		task.await.unwrap().unwrap();
		assert!(scopes.records().is_empty());
		assert_eq!(scopes.notifications(), 1);
	}

	#[tokio::test]
	async fn audit_insert_is_retried_before_a_connection_is_admitted() {
		let memory = Arc::new(MemoryScopes::new("token".into(), test_settings()));
		let scopes = Arc::new(StalledChecks(
			memory.clone(),
			std::sync::atomic::AtomicUsize::new(1),
		));
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = listener.local_addr().unwrap();
		let (stop, receiver) = tokio::sync::watch::channel(false);
		let task = tokio::spawn(super::super::serve_with_scopes(
			listener,
			Arc::new(DenyLegacy),
			Some(scopes),
			super::super::ProxyLimits::default(),
			receiver,
		));
		let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
		client.write_all(b"CONNECT registry.npmjs.org:22 HTTP/1.1\r\nHost: registry.npmjs.org:22\r\nProxy-Authorization: Bearer token\r\nConnection: close\r\n\r\n").await.unwrap();
		let mut response = String::new();
		client.read_to_string(&mut response).await.unwrap();
		assert!(response.contains("invalid_proxy_target"));
		stop.send(true).unwrap();
		task.await.unwrap().unwrap();
		assert_eq!(memory.records().len(), 1);
		assert_eq!(memory.records()[0].outcome, "invalid_proxy_target");
	}

	#[tokio::test]
	async fn invalid_port_is_audited_before_shutdown_finishes() {
		let scopes = Arc::new(MemoryScopes::new("token".into(), test_settings()));
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = listener.local_addr().unwrap();
		let (stop, receiver) = tokio::sync::watch::channel(false);
		let task = tokio::spawn(super::super::serve_with_scopes(
			listener,
			Arc::new(DenyLegacy),
			Some(scopes.clone()),
			super::super::ProxyLimits::default(),
			receiver,
		));
		for port in [22, 0] {
			let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
			client.write_all(format!("CONNECT registry.npmjs.org:{port} HTTP/1.1\r\nHost: registry.npmjs.org:{port}\r\nProxy-Authorization: Bearer token\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
			let mut response = String::new();
			client.read_to_string(&mut response).await.unwrap();
			assert!(response.contains("invalid_proxy_target"));
		}

		stop.send(true).unwrap();
		task.await.unwrap().unwrap();
		let records = scopes.records();
		assert_eq!(records.len(), 2);
		for (record, port) in records.iter().zip([22, 0]) {
			assert_eq!(record.host, "registry.npmjs.org");
			assert_eq!(record.port, port);
			assert_eq!(record.outcome, "invalid_proxy_target");
		}
	}

	#[tokio::test]
	async fn a_refused_proxied_upload_reaches_a_client_that_sends_everything_first() {
		let scopes = Arc::new(MemoryScopes::new("token".into(), test_settings()));
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = listener.local_addr().unwrap();
		let (stop, receiver) = tokio::sync::watch::channel(false);
		let server = tokio::spawn(super::super::serve_with_scopes(
			listener,
			Arc::new(DenyLegacy),
			Some(scopes),
			super::super::ProxyLimits::default(),
			receiver,
		));
		let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
		let length = 64 * 64 * 1024;
		client.write_all(format!("POST http://denied.example/upload HTTP/1.1\r\nHost: denied.example\r\nProxy-Authorization: Bearer token\r\nContent-Length: {length}\r\n\r\n").as_bytes()).await.unwrap();
		client.write_all(&vec![1; length]).await.unwrap();
		let mut response = String::new();
		client.read_to_string(&mut response).await.unwrap();
		assert!(response.starts_with("HTTP/1.1 403"), "{response}");
		assert!(response.contains("egress_host_forbidden"), "{response}");
		stop.send(true).unwrap();
		server.await.unwrap().unwrap();
	}

	#[tokio::test]
	async fn absolute_http_forwards_without_proxy_credentials_and_records_wire_bytes() {
		use hyper::{Request, Response, service::service_fn};
		use std::convert::Infallible;
		let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let upstream_address = upstream.local_addr().unwrap();
		let upstream_task = tokio::spawn(async move {
			let (socket, _) = upstream.accept().await.unwrap();
			let service = service_fn(|request: Request<hyper::body::Incoming>| async move {
				assert!(request.headers().get("proxy-authorization").is_none());
				assert_eq!(
					request.uri().path_and_query().unwrap().as_str(),
					"/private?secret=value"
				);
				Ok::<_, Infallible>(Response::new(http_body_util::Full::new(
					bytes::Bytes::from_static(b"hello"),
				)))
			});
			hyper::server::conn::http1::Builder::new()
				.serve_connection(hyper_util::rt::TokioIo::new(socket), service)
				.await
				.unwrap();
		});
		let settings = ScopeSettings {
			id: ScopeId {
				tenant: "t".into(),
				node: "n".into(),
				scratch_space: uuid::Uuid::new_v4(),
			},
			allowed_hosts: AllowedHosts::default(),
			budget: DailyBudget::default(),
			anomaly: AnomalyRule::default(),
			operator_allows_all: false,
			tunnel_limits: Default::default(),
		};
		let scopes = Arc::new(MemoryScopes::new("token".into(), settings.clone()));
		let gateway = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let gateway_address = gateway.local_addr().unwrap();
		let gateway_scopes = scopes.clone();
		let gateway_task = tokio::spawn(async move {
			let (socket, _) = gateway.accept().await.unwrap();
			let (_stop, receiver) = tokio::sync::watch::channel(false);
			let manager = Arc::new(super::TunnelManager::new(
				receiver,
				super::super::ProxyLimits::default(),
			));
			let service_manager = manager.clone();
			let service = service_fn(move |request| {
				let manager = service_manager.clone();
				let scopes = gateway_scopes.clone();
				let settings = settings.clone();
				async move {
					let audit = super::Audit {
						id: uuid::Uuid::new_v4(),
						manager: manager.clone(),
						scopes: scopes.clone(),
						scope: settings.id.clone(),
						host: "test.invalid".into(),
						port: 80,
						started: std::time::Instant::now(),
						sent: Arc::new(super::AtomicU64::new(0)),
						received: Arc::new(super::AtomicU64::new(0)),
						outcome: "refused",
						finished: false,
						started_record: false,
					};
					let permit = Arc::new(manager.acquire(&settings).unwrap());
					let result = super::forward_http(
						request,
						url::Url::parse("http://test.invalid/private?secret=value").unwrap(),
						vec![upstream_address],
						scopes,
						settings,
						"token".into(),
						audit,
						super::TunnelLease { permit, manager },
					)
					.await;
					Ok::<_, Infallible>(result)
				}
			});
			hyper::server::conn::http1::Builder::new()
				.serve_connection(hyper_util::rt::TokioIo::new(socket), service)
				.await
				.unwrap();
			manager.drain().await;
		});
		let mut client = tokio::net::TcpStream::connect(gateway_address)
			.await
			.unwrap();
		client.write_all(b"GET http://test.invalid/private?secret=value HTTP/1.1\r\nHost: test.invalid\r\nProxy-Authorization: Bearer token\r\nConnection: close\r\n\r\n").await.unwrap();
		let mut response = Vec::new();
		client.read_to_end(&mut response).await.unwrap();
		assert!(
			response.ends_with(b"hello"),
			"response: {}",
			String::from_utf8_lossy(&response)
		);
		upstream_task.await.unwrap();
		gateway_task.await.unwrap();
		tokio::time::timeout(std::time::Duration::from_secs(3), async {
			while scopes.records().is_empty() {
				tokio::task::yield_now().await;
			}
		})
		.await
		.unwrap();
		let records = scopes.records();
		assert_eq!(records[0].host, "test.invalid");
		assert!(!format!("{:?}", records[0]).contains("token"));
		assert!(!format!("{:?}", records[0]).contains("secret=value"));
		assert!(records[0].sent_bytes > 5);
		assert!(records[0].received_bytes > 5);
	}
	#[tokio::test]
	async fn interrupted_close_delimited_http_response_does_not_end_cleanly() {
		exercise_close_delimited_response(true).await;
	}
	#[tokio::test]
	async fn complete_close_delimited_http_response_ends_cleanly() {
		exercise_close_delimited_response(false).await;
	}
	async fn exercise_close_delimited_response(interrupt: bool) {
		use hyper::service::service_fn;
		use std::convert::Infallible;
		let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let upstream_address = upstream.local_addr().unwrap();
		let upstream_task = tokio::spawn(async move {
			let (mut socket, _) = upstream.accept().await.unwrap();
			let mut request = [0; 4096];
			let _ = socket.read(&mut request).await.unwrap();
			socket
				.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\npartial")
				.await
				.unwrap();
			if interrupt {
				std::future::pending::<()>().await;
			}
		});
		let settings = test_settings();
		let scopes = Arc::new(MemoryScopes::new("token".into(), settings.clone()));
		let gateway = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = gateway.local_addr().unwrap();
		let (stop, receiver) = tokio::sync::watch::channel(false);
		let manager = Arc::new(super::TunnelManager::new(
			receiver,
			super::super::ProxyLimits::default(),
		));
		let gateway_manager = manager.clone();
		let task = tokio::spawn(async move {
			let (socket, _) = gateway.accept().await.unwrap();
			let service = service_fn(move |request| {
				let manager = gateway_manager.clone();
				let scopes = scopes.clone();
				let settings = settings.clone();
				async move {
					let audit = super::Audit {
						id: uuid::Uuid::new_v4(),
						manager: manager.clone(),
						scopes: scopes.clone(),
						scope: settings.id.clone(),
						host: "registry.npmjs.org".into(),
						port: 80,
						started: std::time::Instant::now(),
						sent: Arc::new(super::AtomicU64::new(0)),
						received: Arc::new(super::AtomicU64::new(0)),
						outcome: "refused",
						finished: false,
						started_record: false,
					};
					let permit = Arc::new(manager.acquire(&settings).unwrap());
					Ok::<_, Infallible>(
						super::forward_http(
							request,
							url::Url::parse("http://registry.npmjs.org/file").unwrap(),
							vec![upstream_address],
							scopes,
							settings,
							"token".into(),
							audit,
							super::TunnelLease { permit, manager },
						)
						.await,
					)
				}
			});
			let _ = hyper::server::conn::http1::Builder::new()
				.serve_connection(hyper_util::rt::TokioIo::new(socket), service)
				.await;
		});
		let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
		client.write_all(b"GET http://registry.npmjs.org/file HTTP/1.1\r\nHost: registry.npmjs.org\r\nConnection: close\r\n\r\n").await.unwrap();
		let mut response = Vec::new();
		let mut bytes = [0; 1024];
		while !response.windows(7).any(|part| part == b"partial") {
			let n =
				tokio::time::timeout(std::time::Duration::from_secs(5), client.read(&mut bytes))
					.await
					.unwrap()
					.unwrap();
			assert!(n > 0);
			response.extend_from_slice(&bytes[..n]);
		}
		if interrupt {
			stop.send(true).unwrap();
		}
		let read = tokio::time::timeout(
			std::time::Duration::from_secs(5),
			client.read_to_end(&mut response),
		)
		.await
		.unwrap();
		if !interrupt {
			read.unwrap();
		} else if let Err(error) = read {
			assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset);
		}
		assert!(
			String::from_utf8_lossy(&response)
				.to_ascii_lowercase()
				.contains("transfer-encoding: chunked")
		);
		assert_eq!(response.ends_with(b"0\r\n\r\n"), !interrupt);
		if !interrupt {
			stop.send(true).unwrap();
		}
		task.await.unwrap();
		manager.drain().await;
		upstream_task.abort();
	}

	#[tokio::test]
	async fn shutdown_drains_upgraded_tunnel_and_persists_usage_and_record() {
		exercise_tunnel_termination(false).await;
	}
	#[tokio::test]
	async fn aborted_tunnel_records_live_counters_and_interruption() {
		exercise_tunnel_termination(true).await;
	}
	async fn exercise_tunnel_termination(abort: bool) {
		use hyper::service::service_fn;
		use std::convert::Infallible;
		let settings = test_settings();
		let scopes = Arc::new(MemoryScopes::new(
			"secret-credential".into(),
			settings.clone(),
		));
		let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let upstream_address = upstream.local_addr().unwrap();
		let gateway = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = gateway.local_addr().unwrap();
		let (stop, receiver) = tokio::sync::watch::channel(false);
		let manager = Arc::new(super::TunnelManager::new(
			receiver,
			super::super::ProxyLimits::default(),
		));
		let gateway_manager = manager.clone();
		let gateway_scopes = scopes.clone();
		let gateway_task = tokio::spawn(async move {
			let (socket, _) = gateway.accept().await.unwrap();
			let service = service_fn(move |request| {
				let manager = gateway_manager.clone();
				let scopes = gateway_scopes.clone();
				let settings = settings.clone();
				async move {
					let audit = super::Audit {
						id: uuid::Uuid::new_v4(),
						manager: manager.clone(),
						scopes: scopes.clone(),
						scope: settings.id.clone(),
						host: "registry.npmjs.org".into(),
						port: 443,
						started: std::time::Instant::now(),
						sent: Arc::new(super::AtomicU64::new(0)),
						received: Arc::new(super::AtomicU64::new(0)),
						outcome: "refused",
						finished: false,
						started_record: false,
					};
					let permit = Arc::new(manager.acquire(&settings).unwrap());
					let upstream = tokio::net::TcpStream::connect(upstream_address)
						.await
						.unwrap();
					Ok::<_, Infallible>(
						super::connect_tunnel(
							request,
							upstream,
							scopes,
							settings,
							"secret-credential".into(),
							audit,
							permit,
							manager,
						)
						.await,
					)
				}
			});
			hyper::server::conn::http1::Builder::new()
				.serve_connection(hyper_util::rt::TokioIo::new(socket), service)
				.with_upgrades()
				.await
				.unwrap();
		});
		let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
		client
			.write_all(
				b"CONNECT registry.npmjs.org:443 HTTP/1.1\r\nHost: registry.npmjs.org:443\r\n\r\n",
			)
			.await
			.unwrap();
		let mut head = Vec::new();
		while !head.ends_with(b"\r\n\r\n") {
			head.push(client.read_u8().await.unwrap());
		}
		assert!(head.starts_with(b"HTTP/1.1 200"));
		let (mut remote, _) = upstream.accept().await.unwrap();
		client.write_all(b"hello").await.unwrap();
		let mut buffer = [0; 5];
		remote.read_exact(&mut buffer).await.unwrap();
		let live = scopes.records();
		assert_eq!(live.len(), 1);
		assert_eq!(live[0].outcome, "unsettled");
		gateway_task.await.unwrap();
		if abort {
			manager.tasks.lock().unwrap().abort_all();
		} else {
			stop.send(true).unwrap();
		}
		manager.drain().await;
		let usage = scopes.usage(chrono::Utc::now().date_naive()).0;
		if abort {
			assert!(usage >= 5);
		} else {
			assert_eq!(usage, 5);
		}
		let records = scopes.records();
		assert_eq!(records.len(), 1);
		assert_eq!(records[0].sent_bytes, 5);
		assert_eq!(
			records[0].outcome,
			if abort {
				"interrupted"
			} else {
				"egress_shutdown"
			}
		);
		assert!(!format!("{:?}", records[0]).contains("secret-credential"));
	}

	#[tokio::test]
	async fn connect_upgrades_and_counts_tunnel_bytes() {
		use hyper::service::service_fn;
		use std::convert::Infallible;
		let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let upstream_address = upstream.local_addr().unwrap();
		let upstream_task = tokio::spawn(async move {
			let (mut socket, _) = upstream.accept().await.unwrap();
			let mut input = [0; 5];
			socket.read_exact(&mut input).await.unwrap();
			assert_eq!(&input, b"hello");
			socket.write_all(b"world").await.unwrap();
			socket.shutdown().await.unwrap();
		});
		let settings = ScopeSettings {
			id: ScopeId {
				tenant: "t".into(),
				node: "n".into(),
				scratch_space: uuid::Uuid::new_v4(),
			},
			allowed_hosts: AllowedHosts::default(),
			budget: DailyBudget::default(),
			anomaly: AnomalyRule::default(),
			operator_allows_all: false,
			tunnel_limits: Default::default(),
		};
		let scopes = Arc::new(MemoryScopes::new("token".into(), settings.clone()));
		let gateway = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = gateway.local_addr().unwrap();
		let gateway_scopes = scopes.clone();
		let gateway_task = tokio::spawn(async move {
			let (socket, _) = gateway.accept().await.unwrap();
			let (_stop, receiver) = tokio::sync::watch::channel(false);
			let manager = Arc::new(super::TunnelManager::new(
				receiver,
				super::super::ProxyLimits::default(),
			));
			let service_manager = manager.clone();
			let service = service_fn(move |request| {
				let manager = service_manager.clone();
				let scopes = gateway_scopes.clone();
				let settings = settings.clone();
				async move {
					let audit = super::Audit {
						id: uuid::Uuid::new_v4(),
						manager: manager.clone(),
						scopes: scopes.clone(),
						scope: settings.id.clone(),
						host: "test.invalid".into(),
						port: 443,
						started: std::time::Instant::now(),
						sent: Arc::new(super::AtomicU64::new(0)),
						received: Arc::new(super::AtomicU64::new(0)),
						outcome: "refused",
						finished: false,
						started_record: false,
					};
					let upstream = tokio::net::TcpStream::connect(upstream_address)
						.await
						.unwrap();
					let permit = Arc::new(manager.acquire(&settings).unwrap());
					Ok::<_, Infallible>(
						super::connect_tunnel(
							request,
							upstream,
							scopes,
							settings,
							"token".into(),
							audit,
							permit,
							manager,
						)
						.await,
					)
				}
			});
			hyper::server::conn::http1::Builder::new()
				.serve_connection(hyper_util::rt::TokioIo::new(socket), service)
				.with_upgrades()
				.await
				.unwrap();
			manager.drain().await;
		});
		let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
		client
			.write_all(b"CONNECT test.invalid:443 HTTP/1.1\r\nHost: test.invalid:443\r\n\r\n")
			.await
			.unwrap();
		let mut head = Vec::new();
		while !head.ends_with(b"\r\n\r\n") {
			head.push(client.read_u8().await.unwrap());
		}
		assert!(head.starts_with(b"HTTP/1.1 200"));
		client.write_all(b"hello").await.unwrap();
		client.shutdown().await.unwrap();
		let mut reply = [0; 5];
		client.read_exact(&mut reply).await.unwrap();
		assert_eq!(&reply, b"world");
		upstream_task.await.unwrap();
		gateway_task.await.unwrap();
		tokio::time::timeout(std::time::Duration::from_secs(3), async {
			while scopes.records().is_empty() {
				tokio::task::yield_now().await;
			}
		})
		.await
		.unwrap();
		let records = scopes.records();
		assert_eq!(records[0].sent_bytes, 5);
		assert_eq!(records[0].received_bytes, 5);
		assert_eq!(records[0].outcome, "completed");
	}

	/// A store that fails the chosen calls, repeating what it was given in the
	/// error's text as a database error can.
	struct FaultyScopes {
		memory: Arc<MemoryScopes>,
		resolve_fails: bool,
		debit_fails: bool,
		refund_fails: bool,
		record_hangs: bool,
		record_fails:
			Box<dyn Fn(&aidash_application::egress::ConnectionRecord) -> bool + Send + Sync>,
	}
	impl FaultyScopes {
		fn new(settings: &ScopeSettings) -> Self {
			Self {
				memory: Arc::new(MemoryScopes::new(CREDENTIAL.into(), settings.clone())),
				resolve_fails: false,
				debit_fails: false,
				refund_fails: false,
				record_hangs: false,
				record_fails: Box::new(|_| false),
			}
		}
	}
	#[async_trait::async_trait]
	impl EgressScopes for FaultyScopes {
		async fn resolve(&self, c: &str) -> aidash_application::Result<ScopeSettings> {
			if self.resolve_fails {
				return Err(aidash_application::Error::External(format!(
					"pool timed out resolving {c}"
				)));
			}
			self.memory.resolve(c).await
		}
		async fn admit_external(
			&self,
			c: &str,
			s: &str,
			m: &str,
			p: &str,
			d: &str,
		) -> aidash_application::Result<aidash_application::egress::Admission> {
			self.memory.admit_external(c, s, m, p, d).await
		}
		async fn admit_connection(&self, s: &ScopeSettings) -> aidash_application::Result<()> {
			self.memory.admit_connection(s).await
		}
		async fn check_active(
			&self,
			s: &ScopeSettings,
			c: &str,
		) -> aidash_application::Result<ScopeSettings> {
			self.memory.check_active(s, c).await
		}
		async fn debit(
			&self,
			s: &ScopeSettings,
			d: aidash_application::egress::Direction,
			n: u64,
			day: chrono::NaiveDate,
		) -> aidash_application::Result<()> {
			if self.debit_fails {
				return Err(aidash_application::Error::External(
					"pool timed out debiting".into(),
				));
			}
			self.memory.debit(s, d, n, day).await
		}
		async fn refund(
			&self,
			s: &ScopeSettings,
			d: aidash_application::egress::Direction,
			n: u64,
			day: chrono::NaiveDate,
		) -> aidash_application::Result<()> {
			if self.refund_fails {
				return Err(aidash_application::Error::External(
					"pool timed out refunding".into(),
				));
			}
			self.memory.refund(s, d, n, day).await
		}
		async fn record(
			&self,
			r: aidash_application::egress::ConnectionRecord,
		) -> aidash_application::Result<()> {
			if self.record_hangs {
				return std::future::pending().await;
			}
			if (self.record_fails)(&r) {
				return Err(aidash_application::Error::External(format!(
					"pool timed out recording {}",
					r.host
				)));
			}
			self.memory.record(r).await
		}
		async fn lift_suspension(&self, id: &ScopeId) -> aidash_application::Result<()> {
			self.memory.lift_suspension(id).await
		}
	}

	const CREDENTIAL: &str = "canary-credential";
	const HOST: &str = "canary-host.invalid";
	const FAILED: &str = "Egress host connection failed";

	fn failures(kind: &str) -> String {
		format!(r#"aidash_egress_host_connection_failures_total{{kind="{kind}"}}"#)
	}

	fn assert_private(logs: &str) {
		for private in [CREDENTIAL, HOST, "private-path"] {
			assert!(!logs.contains(private), "{private} in {logs}");
		}
	}

	fn lines_at<'a>(logs: &'a str, level: &str, kind: &str) -> Vec<&'a str> {
		logs.lines()
			.filter(|line| line.contains(level) && line.contains(&format!("kind=\"{kind}\"")))
			.collect()
	}

	fn audit_for(
		manager: &Arc<super::TunnelManager>,
		scopes: Arc<dyn EgressScopes>,
		settings: &ScopeSettings,
	) -> super::Audit {
		super::Audit {
			id: uuid::Uuid::new_v4(),
			manager: manager.clone(),
			scopes,
			scope: settings.id.clone(),
			host: HOST.into(),
			port: 80,
			started: std::time::Instant::now(),
			sent: Arc::new(super::AtomicU64::new(0)),
			received: Arc::new(super::AtomicU64::new(0)),
			outcome: "refused",
			finished: false,
			started_record: false,
		}
	}

	/// Sends one plain HTTP request through `forward_http` and returns the raw
	/// response once the connection has ended.
	async fn forward_once(
		manager: &Arc<super::TunnelManager>,
		scopes: Arc<dyn EgressScopes>,
		settings: &ScopeSettings,
		addresses: Vec<std::net::SocketAddr>,
	) -> String {
		use hyper::service::service_fn;
		use std::convert::Infallible;
		let gateway = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = gateway.local_addr().unwrap();
		let manager = manager.clone();
		let settings = settings.clone();
		let served = tokio::spawn(async move {
			let (socket, _) = gateway.accept().await.unwrap();
			let service = service_fn(move |request| {
				let manager = manager.clone();
				let scopes = scopes.clone();
				let settings = settings.clone();
				let addresses = addresses.clone();
				async move {
					let audit = audit_for(&manager, scopes.clone(), &settings);
					let permit = Arc::new(manager.acquire(&settings).unwrap());
					let result = super::forward_http(
						request,
						url::Url::parse(&format!("http://{HOST}/private-path")).unwrap(),
						addresses,
						scopes,
						settings,
						CREDENTIAL.into(),
						audit,
						super::TunnelLease { permit, manager },
					)
					.await;
					Ok::<_, Infallible>(result)
				}
			});
			let _ = hyper::server::conn::http1::Builder::new()
				.serve_connection(hyper_util::rt::TokioIo::new(socket), service)
				.await;
		});
		let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
		client
			.write_all(
				format!(
					"GET http://{HOST}/private-path HTTP/1.1\r\nHost: {HOST}\r\nConnection: close\r\n\r\n"
				)
				.as_bytes(),
			)
			.await
			.unwrap();
		let mut response = Vec::new();
		let _ = client.read_to_end(&mut response).await;
		served.await.unwrap();
		String::from_utf8_lossy(&response).into_owned()
	}

	async fn closed_address() -> std::net::SocketAddr {
		// Nothing listens on a port that was just released.
		let unused = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		unused.local_addr().unwrap()
	}

	fn manager() -> (tokio::sync::watch::Sender<bool>, Arc<super::TunnelManager>) {
		let (stop, receiver) = tokio::sync::watch::channel(false);
		let manager = super::TunnelManager::new(receiver, super::super::ProxyLimits::default());
		(stop, Arc::new(manager))
	}

	async fn proxy_response(scopes: Arc<dyn EgressScopes>, request: String) -> String {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = listener.local_addr().unwrap();
		let (stop, receiver) = tokio::sync::watch::channel(false);
		let task = tokio::spawn(super::super::serve_with_scopes(
			listener,
			Arc::new(DenyLegacy),
			Some(scopes),
			super::super::ProxyLimits::default(),
			receiver,
		));
		let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
		client.write_all(request.as_bytes()).await.unwrap();
		let mut response = String::new();
		client.read_to_string(&mut response).await.unwrap();
		stop.send(true).unwrap();
		task.await.unwrap().unwrap();
		response
	}

	#[tokio::test]
	async fn a_refused_upstream_connection_leaves_its_cause_without_host_or_credential() {
		let observed = super::super::tests::Observed::start();
		let settings = test_settings();
		let scopes = Arc::new(MemoryScopes::new(CREDENTIAL.into(), settings.clone()));
		let (_stop, manager) = manager();
		let closed = closed_address().await;
		let response = forward_once(&manager, scopes.clone(), &settings, vec![closed]).await;
		assert!(response.contains("egress_connect_failed"), "{response}");
		manager.drain().await;
		assert_eq!(scopes.records()[0].outcome, "connect_error");
		let logs = observed.logs();
		assert_eq!(lines_at(&logs, "WARN", "connect").len(), 1, "{logs}");
		assert!(logs.contains(FAILED), "{logs}");
		assert!(logs.to_lowercase().contains("refused"), "{logs}");
		assert!(logs.contains(&scopes.records()[0].id.to_string()), "{logs}");
		assert_private(&logs);
		assert_eq!(observed.count(&failures("connect")), 1);
	}

	#[tokio::test]
	async fn repeated_upstream_failures_log_one_line_and_count_every_one() {
		let observed = super::super::tests::Observed::start();
		let settings = test_settings();
		let scopes = Arc::new(MemoryScopes::new(CREDENTIAL.into(), settings.clone()));
		let (_stop, manager) = manager();
		let closed = closed_address().await;
		for _ in 0..5 {
			forward_once(&manager, scopes.clone(), &settings, vec![closed]).await;
		}
		manager.drain().await;
		let logs = observed.logs();
		assert_eq!(lines_at(&logs, "WARN", "connect").len(), 1, "{logs}");
		assert_eq!(lines_at(&logs, "DEBUG", "connect").len(), 4, "{logs}");
		assert_eq!(observed.count(&failures("connect")), 5);
	}

	#[tokio::test]
	async fn an_upstream_closing_without_a_response_leaves_the_cause_of_the_request() {
		let observed = super::super::tests::Observed::start();
		let settings = test_settings();
		let scopes = Arc::new(MemoryScopes::new(CREDENTIAL.into(), settings.clone()));
		let (_stop, manager) = manager();
		let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = upstream.local_addr().unwrap();
		let upstream_task = tokio::spawn(async move {
			let (mut socket, _) = upstream.accept().await.unwrap();
			let mut request = [0; 4096];
			let _ = socket.read(&mut request).await.unwrap();
		});
		let response = forward_once(&manager, scopes.clone(), &settings, vec![address]).await;
		assert!(response.contains("egress_upstream_failed"), "{response}");
		upstream_task.await.unwrap();
		manager.drain().await;
		let logs = observed.logs();
		assert_eq!(lines_at(&logs, "WARN", "request").len(), 1, "{logs}");
		assert!(logs.contains("closed"), "{logs}");
		assert_private(&logs);
		assert_eq!(observed.count(&failures("request")), 1);
	}

	#[tokio::test]
	async fn a_failed_upgrade_leaves_its_cause() {
		use hyper::service::service_fn;
		use std::convert::Infallible;
		let observed = super::super::tests::Observed::start();
		let settings = test_settings();
		let scopes = Arc::new(MemoryScopes::new(CREDENTIAL.into(), settings.clone()));
		let (_stop, manager) = manager();
		let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let upstream_address = upstream.local_addr().unwrap();
		let gateway = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = gateway.local_addr().unwrap();
		let (service_manager, service_scopes) = (manager.clone(), scopes.clone());
		let served = tokio::spawn(async move {
			let (socket, _) = gateway.accept().await.unwrap();
			let service = service_fn(move |request| {
				let manager = service_manager.clone();
				let scopes = service_scopes.clone();
				let settings = settings.clone();
				async move {
					let scopes: Arc<dyn EgressScopes> = scopes;
					let mut audit = audit_for(&manager, scopes.clone(), &settings);
					audit.started_record = true;
					let permit = Arc::new(manager.acquire(&settings).unwrap());
					let upstream = tokio::net::TcpStream::connect(upstream_address)
						.await
						.unwrap();
					let result = super::connect_tunnel(
						request,
						upstream,
						scopes,
						settings,
						CREDENTIAL.into(),
						audit,
						permit,
						manager,
					)
					.await;
					Ok::<_, Infallible>(result)
				}
			});
			let _ = hyper::server::conn::http1::Builder::new()
				.serve_connection(hyper_util::rt::TokioIo::new(socket), service)
				.with_upgrades()
				.await;
		});
		let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
		// A plain request never upgrades, so the wait for it fails when the
		// connection ends.
		client
			.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
			.await
			.unwrap();
		let mut response = Vec::new();
		let _ = client.read_to_end(&mut response).await;
		served.await.unwrap();
		manager.drain().await;
		assert_eq!(scopes.records()[0].outcome, "upgrade_failed");
		let logs = observed.logs();
		assert_eq!(lines_at(&logs, "WARN", "upgrade").len(), 1, "{logs}");
		assert_private(&logs);
		assert_eq!(observed.count(&failures("upgrade")), 1);
	}

	#[tokio::test]
	async fn a_failed_name_lookup_leaves_its_cause_without_the_host() {
		let observed = super::super::tests::Observed::start();
		let mut settings = test_settings();
		settings.allowed_hosts.hosts = vec![HOST.into()];
		let scopes = Arc::new(MemoryScopes::new(CREDENTIAL.into(), settings));
		let response = proxy_response(
			scopes.clone(),
			format!(
				"CONNECT {HOST}:443 HTTP/1.1\r\nHost: {HOST}:443\r\nProxy-Authorization: Bearer {CREDENTIAL}\r\nConnection: close\r\n\r\n"
			),
		)
		.await;
		assert!(response.contains("egress_dns_failed"), "{response}");
		assert_eq!(scopes.records()[0].outcome, "dns_error");
		let logs = observed.logs();
		assert_eq!(lines_at(&logs, "WARN", "dns").len(), 1, "{logs}");
		assert_private(&logs);
		assert_eq!(observed.count(&failures("dns")), 1);
	}

	#[tokio::test]
	async fn connections_through_one_listener_share_one_thinned_line() {
		let observed = super::super::tests::Observed::start();
		let mut settings = test_settings();
		settings.allowed_hosts.hosts = vec![HOST.into()];
		let scopes = Arc::new(MemoryScopes::new(CREDENTIAL.into(), settings));
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = listener.local_addr().unwrap();
		let (stop, receiver) = tokio::sync::watch::channel(false);
		let task = tokio::spawn(super::super::serve_with_scopes(
			listener,
			Arc::new(DenyLegacy),
			Some(scopes.clone()),
			super::super::ProxyLimits::default(),
			receiver,
		));
		for _ in 0..3 {
			let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
			client
				.write_all(
					format!(
						"CONNECT {HOST}:443 HTTP/1.1\r\nHost: {HOST}:443\r\nProxy-Authorization: Bearer {CREDENTIAL}\r\nConnection: close\r\n\r\n"
					)
					.as_bytes(),
				)
				.await
				.unwrap();
			let mut response = String::new();
			client.read_to_string(&mut response).await.unwrap();
			assert!(response.contains("egress_dns_failed"), "{response}");
		}
		stop.send(true).unwrap();
		task.await.unwrap().unwrap();
		let logs = observed.logs();
		assert_eq!(lines_at(&logs, "WARN", "dns").len(), 1, "{logs}");
		assert_eq!(lines_at(&logs, "DEBUG", "dns").len(), 2, "{logs}");
		assert_eq!(observed.count(&failures("dns")), 3);
		assert_eq!(scopes.records().len(), 3);
		assert!(scopes.records().iter().all(|r| r.outcome == "dns_error"));
	}

	#[tokio::test]
	async fn an_unexpected_admission_failure_is_logged_and_counted_without_the_credential() {
		let observed = super::super::tests::Observed::start();
		let mut scopes = FaultyScopes::new(&test_settings());
		scopes.resolve_fails = true;
		let response = proxy_response(
			Arc::new(scopes),
			format!(
				"CONNECT {HOST}:443 HTTP/1.1\r\nHost: {HOST}:443\r\nProxy-Authorization: Bearer {CREDENTIAL}\r\nConnection: close\r\n\r\n"
			),
		)
		.await;
		assert!(response.contains("egress_unavailable"), "{response}");
		let logs = observed.logs();
		assert_eq!(lines_at(&logs, "ERROR", "admission").len(), 1, "{logs}");
		assert!(logs.contains("pool timed out"), "{logs}");
		assert_private(&logs);
		assert_eq!(
			observed.count(r#"aidash_egress_refused_total{reason="egress_unavailable"}"#),
			1
		);
	}

	#[tokio::test]
	async fn refusals_before_a_record_exists_are_counted_by_reason() {
		let observed = super::super::tests::Observed::start();
		let scopes = Arc::new(MemoryScopes::new(CREDENTIAL.into(), test_settings()));
		proxy_response(
			scopes.clone(),
			format!("CONNECT {HOST}:443 HTTP/1.1\r\nHost: {HOST}:443\r\nConnection: close\r\n\r\n"),
		)
		.await;
		proxy_response(
			scopes.clone(),
			format!(
				"CONNECT {HOST}:443 HTTP/1.1\r\nHost: {HOST}:443\r\nProxy-Authorization: Bearer wrong\r\nConnection: close\r\n\r\n"
			),
		)
		.await;
		assert_eq!(
			observed.count(r#"aidash_egress_refused_total{reason="proxy_auth_required"}"#),
			2
		);
		assert!(!observed.logs().contains("ERROR"), "{}", observed.logs());
		assert!(scopes.records().is_empty());
	}

	#[tokio::test]
	async fn a_start_record_that_cannot_be_written_refuses_and_logs_its_cause() {
		let observed = super::super::tests::Observed::start();
		let mut settings = test_settings();
		settings.allowed_hosts.hosts = vec![HOST.into()];
		let mut scopes = FaultyScopes::new(&settings);
		scopes.record_fails = Box::new(|_| true);
		let response = proxy_response(
			Arc::new(scopes),
			format!(
				"CONNECT {HOST}:443 HTTP/1.1\r\nHost: {HOST}:443\r\nProxy-Authorization: Bearer {CREDENTIAL}\r\nConnection: close\r\n\r\n"
			),
		)
		.await;
		assert!(response.contains("egress_audit_unavailable"), "{response}");
		let logs = observed.logs();
		assert_eq!(lines_at(&logs, "ERROR", "start_record").len(), 1, "{logs}");
		assert!(logs.contains("pool timed out"), "{logs}");
		assert_private(&logs);
		assert_eq!(
			observed.count(r#"aidash_egress_refused_total{reason="egress_audit_unavailable"}"#),
			1
		);
	}

	#[tokio::test]
	async fn the_start_record_is_unsettled_and_an_invalid_target_says_so_from_the_start() {
		let settings = test_settings();
		let scopes = Arc::new(MemoryScopes::new(CREDENTIAL.into(), settings.clone()));
		let (_stop, manager) = manager();
		let mut audit = audit_for(&manager, scopes.clone(), &settings);
		assert!(audit.start().await);
		assert_eq!(scopes.records()[0].outcome, "unsettled");
		audit.finished = true;

		let mut invalid = audit_for(&manager, scopes.clone(), &settings);
		invalid.port = 0;
		invalid.outcome = "invalid_proxy_target";
		assert!(invalid.start().await);
		let records = scopes.records();
		let written = records.iter().find(|record| record.port == 0).unwrap();
		assert_eq!(written.outcome, "invalid_proxy_target");
		invalid.finished = true;
	}

	#[tokio::test(start_paused = true)]
	async fn the_log_is_thinned_per_kind_and_reports_the_number_since_its_last_line() {
		let log = super::FailureLog::default();
		assert_eq!(log.due("connect"), Some(1));
		assert_eq!(log.due("connect"), None);
		assert_eq!(log.due("connect"), None);
		assert_eq!(log.due("dns"), Some(1), "one kind must not hide another");
		tokio::time::advance(std::time::Duration::from_secs(10)).await;
		assert_eq!(log.due("connect"), Some(3));
		assert_eq!(log.due("connect"), None);
	}

	#[tokio::test]
	async fn only_an_end_with_no_named_code_counts_as_a_transfer_failure() {
		let observed = super::super::tests::Observed::start();
		let report = super::FailureReport {
			host: HOST.into(),
			..Default::default()
		};
		let failed = |text: &str| Err(std::io::Error::other(text.to_owned()));
		assert_eq!(super::tunnel_ended(&report, &Ok(())), "completed");
		assert_eq!(
			super::tunnel_ended(&report, &failed("egress_idle_timeout")),
			"egress_idle_timeout"
		);
		assert_eq!(
			super::tunnel_ended(&report, &failed(super::CLIENT_CLOSED)),
			super::CLIENT_CLOSED
		);
		assert_eq!(observed.count(&failures("transfer")), 0);
		assert_eq!(
			super::tunnel_ended(&report, &failed(&format!("reset by {HOST}"))),
			"interrupted"
		);
		assert_eq!(observed.count(&failures("transfer")), 1);
		assert_eq!(
			super::tunnel_ended(
				&report,
				&failed("egress_authority_unavailable: the check timed out")
			),
			"egress_authority_unavailable"
		);
		assert_eq!(observed.count(&failures("transfer")), 1);
		let logs = observed.logs();
		assert!(logs.contains("reset by [host]"), "{logs}");
		assert_eq!(lines_at(&logs, "ERROR", "authority").len(), 1, "{logs}");
		assert!(logs.contains("the check timed out"), "{logs}");
		assert_private(&logs);
	}

	/// Closing with linger 0 sends a reset rather than a normal close.
	fn reset(socket: tokio::net::TcpStream) {
		socket2::SockRef::from(&socket)
			.set_linger(Some(std::time::Duration::ZERO))
			.unwrap();
		drop(socket);
	}

	async fn socket_pair() -> (tokio::net::TcpStream, tokio::net::TcpStream) {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let near = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
			.await
			.unwrap();
		let (far, _) = listener.accept().await.unwrap();
		(near, far)
	}

	#[tokio::test]
	async fn a_reset_tells_which_side_went_away() {
		let (_stop, receiver) = tokio::sync::watch::channel(false);
		let settings = test_settings();
		let scopes = Arc::new(MemoryScopes::new(CREDENTIAL.into(), settings.clone()));
		for (client_resets, outcome) in [(true, super::CLIENT_CLOSED), (false, "interrupted")] {
			let (gateway_client, client) = socket_pair().await;
			let (gateway_upstream, upstream) = socket_pair().await;
			let task = tokio::spawn(super::tunnel_controlled(
				gateway_client,
				gateway_upstream,
				scopes.clone(),
				settings.clone(),
				CREDENTIAL.into(),
				HOST.into(),
				super::TunnelReports::default(),
				receiver.clone(),
			));
			let (gone, stays) = if client_resets {
				(client, upstream)
			} else {
				(upstream, client)
			};
			reset(gone);
			let result = task.await.unwrap().2;
			drop(stays);
			assert_eq!(super::tunnel_outcome(&result), outcome, "{result:?}");
		}
	}

	#[tokio::test(start_paused = true)]
	async fn a_failed_refund_leaves_its_cause_and_is_not_an_interruption() {
		let observed = super::super::tests::Observed::start();
		let settings = test_settings();
		let mut scopes = FaultyScopes::new(&settings);
		scopes.refund_fails = true;
		let (mut client, gateway_client) = tokio::io::duplex(64);
		let (gateway_server, mut server) = tokio::io::duplex(64);
		let (_stop, receiver) = tokio::sync::watch::channel(false);
		let task = tokio::spawn(super::tunnel_controlled(
			gateway_client,
			gateway_server,
			Arc::new(scopes),
			settings,
			CREDENTIAL.into(),
			HOST.into(),
			super::TunnelReports::default(),
			receiver,
		));
		client.write_all(b"hello").await.unwrap();
		let mut received = [0; 5];
		server.read_exact(&mut received).await.unwrap();
		drop(client);
		drop(server);
		let (_, _, result) = task.await.unwrap();
		assert_eq!(super::tunnel_outcome(&result), "egress_refund_failed");
		let logs = observed.logs();
		assert_eq!(lines_at(&logs, "ERROR", "refund").len(), 1, "{logs}");
		assert!(logs.contains("pool timed out refunding"), "{logs}");
		assert_eq!(
			observed.count(
				r#"aidash_egress_settle_failures_total{stage="refund",outcome="abandoned"}"#
			),
			1
		);
	}

	/// A store that cannot be asked is our own failure, not the upstream's.
	#[tokio::test(start_paused = true)]
	async fn a_failed_debit_is_the_authority_s_failure_and_not_an_upstream_transfer() {
		let observed = super::super::tests::Observed::start();
		let settings = test_settings();
		let mut scopes = FaultyScopes::new(&settings);
		scopes.debit_fails = true;
		let (mut client, gateway_client) = tokio::io::duplex(64);
		let (gateway_server, _server) = tokio::io::duplex(64);
		let (_stop, receiver) = tokio::sync::watch::channel(false);
		let reports = super::TunnelReports::default();
		let report = reports.failure.clone();
		let task = tokio::spawn(super::tunnel_controlled(
			gateway_client,
			gateway_server,
			Arc::new(scopes),
			settings,
			CREDENTIAL.into(),
			HOST.into(),
			reports,
			receiver,
		));
		client.write_all(b"hello").await.unwrap();
		let (_, _, result) = task.await.unwrap();
		assert_eq!(
			super::tunnel_ended(&report, &result),
			"egress_authority_unavailable",
			"{result:?}"
		);
		assert_eq!(observed.count(&failures("transfer")), 0);
		let logs = observed.logs();
		assert_eq!(lines_at(&logs, "ERROR", "authority").len(), 1, "{logs}");
		assert!(logs.contains("pool timed out debiting"), "{logs}");
		assert!(lines_at(&logs, "WARN", "transfer").is_empty(), "{logs}");
	}

	/// A tunnel that crosses UTC midnight refunds the day before it, and that
	/// refund is the same store failure as the one that ends a tunnel.
	#[tokio::test]
	async fn a_refund_that_fails_when_the_day_rolls_is_named_and_counted() {
		let observed = super::super::tests::Observed::start();
		let settings = test_settings();
		let mut scopes = FaultyScopes::new(&settings);
		scopes.refund_fails = true;
		let report = report_for(HOST);
		let mut batch = super::Batch::new(
			Arc::new(scopes),
			settings,
			aidash_application::egress::Direction::Sent,
			report.clone(),
		);
		batch.day = batch.day.pred_opt().unwrap();
		batch.remaining = 10;
		let result = batch.charge(1).await;
		assert_eq!(
			super::tunnel_ended(&report, &result),
			"egress_refund_failed",
			"{result:?}"
		);
		assert_eq!(observed.count(&failures("transfer")), 0);
		assert_eq!(
			observed.count(
				r#"aidash_egress_settle_failures_total{stage="refund",outcome="abandoned"}"#
			),
			1
		);
		let logs = observed.logs();
		assert_eq!(lines_at(&logs, "ERROR", "refund").len(), 1, "{logs}");
		assert!(logs.contains("pool timed out refunding"), "{logs}");
	}

	fn record(settings: &ScopeSettings) -> aidash_application::egress::ConnectionRecord {
		aidash_application::egress::ConnectionRecord {
			id: uuid::Uuid::new_v4(),
			scope: settings.id.clone(),
			host: HOST.into(),
			port: 443,
			sent_bytes: 0,
			received_bytes: 0,
			duration_ms: 0,
			outcome: "completed".into(),
		}
	}

	fn report_for(host: &str) -> super::FailureReport {
		super::FailureReport {
			host: host.into(),
			..Default::default()
		}
	}

	const RETRIED: &str =
		r#"aidash_egress_settle_failures_total{stage="connection_record",outcome="retried"}"#;
	const ABANDONED: &str =
		r#"aidash_egress_settle_failures_total{stage="connection_record",outcome="abandoned"}"#;

	#[tokio::test(start_paused = true)]
	async fn the_final_record_outlasts_the_stall_seen_in_709() {
		let observed = super::super::tests::Observed::start();
		let settings = test_settings();
		let mut scopes = FaultyScopes::new(&settings);
		let began = tokio::time::Instant::now();
		scopes.record_fails =
			Box::new(move |_| began.elapsed() < std::time::Duration::from_secs(47));
		let memory = scopes.memory.clone();
		let (_stop, receiver) = tokio::sync::watch::channel(false);
		super::settle_record(
			Arc::new(scopes),
			record(&settings),
			receiver,
			report_for(HOST),
		)
		.await;
		assert!(began.elapsed() >= std::time::Duration::from_secs(47));
		assert_eq!(memory.records().len(), 1);
		assert!(observed.count(RETRIED) > 0);
		assert_eq!(observed.count(ABANDONED), 0);
		assert!(!observed.logs().contains("ERROR"), "{}", observed.logs());
	}

	#[tokio::test(start_paused = true)]
	async fn the_final_record_gives_up_after_about_a_minute_and_says_why() {
		let observed = super::super::tests::Observed::start();
		let settings = test_settings();
		let mut scopes = FaultyScopes::new(&settings);
		scopes.record_fails = Box::new(|_| true);
		let began = tokio::time::Instant::now();
		let (_stop, receiver) = tokio::sync::watch::channel(false);
		super::settle_record(
			Arc::new(scopes),
			record(&settings),
			receiver,
			report_for(HOST),
		)
		.await;
		assert!(began.elapsed() >= std::time::Duration::from_secs(60));
		assert!(began.elapsed() < std::time::Duration::from_secs(80));
		assert_eq!(observed.count(ABANDONED), 1);
		let logs = observed.logs();
		assert_eq!(lines_at(&logs, "ERROR", "final_record").len(), 1, "{logs}");
		assert!(logs.contains("pool timed out recording [host]"), "{logs}");
		assert_private(&logs);
	}

	#[tokio::test(start_paused = true)]
	async fn a_stop_during_the_wait_leaves_one_more_attempt() {
		let observed = super::super::tests::Observed::start();
		let settings = test_settings();
		let mut scopes = FaultyScopes::new(&settings);
		let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
		let counted = attempts.clone();
		scopes.record_fails = Box::new(move |_| {
			counted.fetch_add(1, Ordering::SeqCst);
			true
		});
		let (stop, receiver) = tokio::sync::watch::channel(false);
		let began = tokio::time::Instant::now();
		let task = tokio::spawn(super::settle_record(
			Arc::new(scopes),
			record(&settings),
			receiver,
			report_for(HOST),
		));
		tokio::task::yield_now().await;
		assert_eq!(attempts.load(Ordering::SeqCst), 1);
		stop.send(true).unwrap();
		task.await.unwrap();
		assert_eq!(attempts.load(Ordering::SeqCst), 2);
		assert!(began.elapsed() < std::time::Duration::from_secs(1));
		assert_eq!(observed.count(ABANDONED), 1);
	}

	#[tokio::test(start_paused = true)]
	async fn a_process_already_stopping_bounds_its_one_attempt() {
		let observed = super::super::tests::Observed::start();
		let settings = test_settings();
		let mut scopes = FaultyScopes::new(&settings);
		scopes.record_hangs = true;
		let (stop, receiver) = tokio::sync::watch::channel(false);
		stop.send(true).unwrap();
		let began = tokio::time::Instant::now();
		super::settle_record(
			Arc::new(scopes),
			record(&settings),
			receiver,
			report_for(HOST),
		)
		.await;
		assert_eq!(began.elapsed(), super::STOPPING_ATTEMPT);
		assert_eq!(observed.count(ABANDONED), 1);
		assert!(observed.logs().contains("attempt timed out"));
	}

	#[tokio::test]
	async fn a_connection_gives_up_its_slot_while_its_final_record_retries() {
		let settings = test_settings();
		let mut scopes = FaultyScopes::new(&settings);
		scopes.record_fails = Box::new(|record| record.outcome != "unsettled");
		let memory = scopes.memory.clone();
		let (stop, manager) = manager();
		let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = upstream.local_addr().unwrap();
		let upstream_task = tokio::spawn(async move {
			let (mut socket, _) = upstream.accept().await.unwrap();
			let mut request = [0; 4096];
			let _ = socket.read(&mut request).await.unwrap();
			socket
				.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
				.await
				.unwrap();
		});
		let response = forward_once(&manager, Arc::new(scopes), &settings, vec![address]).await;
		assert!(response.ends_with("ok"), "{response}");
		upstream_task.await.unwrap();
		tokio::time::timeout(std::time::Duration::from_secs(3), async {
			while !manager.counts.lock().unwrap().scopes.is_empty() {
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
			}
		})
		.await
		.expect("the slot must be free while the record is still retrying");
		assert_eq!(memory.records()[0].outcome, "unsettled");
		stop.send(true).unwrap();
		manager.drain().await;
		assert_eq!(memory.records()[0].outcome, "unsettled");
	}
}
