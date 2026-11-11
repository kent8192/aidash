//! Internal HTTP streaming adapter. The dashboard framework buffers incoming
//! bodies, so this listener uses Hyper directly; it has no public API routes.
use aidash_application::egress::{Admission, Authority, Direction, Meter, RequestTimes};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use bytes::Bytes;
use futures_util::{StreamExt, TryStreamExt};
use http_body_util::{BodyExt, Full, StreamBody, combinators::UnsyncBoxBody};
use hyper::{Request, Response, StatusCode, body::Frame, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use secrecy::ExposeSecret;
use sha2::{Digest, Sha256};
use std::{
	convert::Infallible,
	sync::{
		Arc, OnceLock,
		atomic::{AtomicU64, Ordering},
	},
	time::{Duration, Instant},
};
use tokio::{net::TcpListener, sync::watch};
use url::Url;

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type Body = UnsyncBoxBody<Bytes, BoxError>;
const IDLE: Duration = Duration::from_secs(30);
const INDEX_BYTES: usize = 8 * 1024 * 1024;
const BROKER_ERROR: &str = "x-aidash-broker-error";
const SECRET_CALL: &str = "x-aidash-secret-call";
/// Body bytes are debited in batches. Each debit is a PostgreSQL transaction
/// that rechecks the operation and locks its budget rows and request record,
/// so debiting every chunk held a stream to a few dozen KB/s (#683). At 1 MiB a
/// 1 GiB budget takes at most 1,024 debits per direction, while a stream can
/// exceed its budget by at most one batch plus one chunk before the debit that
/// reaches the limit stops it.
const BATCH_BYTES: u64 = 1024 * 1024;
/// How long a failed or interrupted stream may take to debit its remainder.
const FLUSH_GRACE: Duration = Duration::from_secs(5);
/// How long a finished request keeps trying to settle its last debit and its
/// record in the background, which outlasts a storage stall of about a minute.
/// A record that is still not settled when this runs out, or when the process
/// stops, keeps status 0: its outcome is unknown.
const SETTLE_BUDGET: Duration = Duration::from_secs(60);
/// One settling attempt. Longer than the database pool's 10 s acquire timeout,
/// so that a stalled attempt reports the pool's error instead of this timeout.
const SETTLE_ATTEMPT: Duration = Duration::from_secs(15);
const SETTLE_MAX_DELAY: Duration = Duration::from_secs(15);
/// How long a response waits for its request record to be written before it ends
/// anyway. The write goes on in a task of its own, so a stalled store never holds
/// a client and a client that disconnects never loses the record.
const RECORD_WAIT: Duration = Duration::from_secs(1);
/// The record's error when the final debit ran out of retries: the bytes it held
/// are not in the request's counts or the budgets.
const DEBIT_ABANDONED: &str = "egress_debit_abandoned";
/// The record's error for a body that stopped before its end, unless something
/// said why: a client that went away, or a response cut short.
const STREAM_INTERRUPTED: &str = "stream_interrupted";
/// Connections served at once. Beyond this a connection is refused.
const CONNECTION_SLOTS: usize = 128;
/// Refused connections answered at once, and for how long each. Sandboxed code
/// can open connections freely, so answering them must be bounded too; beyond
/// this a connection is dropped unanswered.
const REFUSAL_SLOTS: usize = 8;
const REFUSAL_WINDOW: Duration = Duration::from_secs(2);
/// Sandboxed code can fill the slots at will, so the log is thinned. The
/// counter stays exact.
const REFUSAL_LOG_INTERVAL: Duration = Duration::from_secs(10);
const INDEX_LIMIT_MESSAGE: &str = "package_index_limit_exceeded: this package index is over 8 MiB. Install from a lockfile, or by tarball URL through the registry.";
const CONNECTION_LIMIT: &str = "egress_connection_limit";
/// How much of a request body may be discarded after the answer, and for how
/// long. These bytes never leave the gateway, so they are not charged.
const LINGER_BYTES: u64 = 8 * 1024 * 1024;
const LINGER_TIME: Duration = Duration::from_secs(5);

fn millis(elapsed: Duration) -> u64 {
	elapsed.as_millis().min(u64::MAX as u128) as u64
}

fn failure(status: StatusCode, code: &'static str) -> Response<Body> {
	Response::builder()
		.status(status)
		.header("content-type", "application/json")
		.header("cache-control", "no-store")
		.body(
			Full::new(Bytes::from(format!("{{\"error\":\"{code}\"}}")))
				.map_err(|never| match never {})
				.boxed_unsync(),
		)
		.unwrap()
}
/// A broker-owned error never carries a value to the sandbox: the gateway
/// reports the code alone (ADR 0095).
/// Refuses a request before its record exists, so that the count is all that
/// is left of it.
fn refuse(status: StatusCode, code: &'static str) -> Response<Body> {
	metrics::counter!("aidash_egress_refused_total", "reason" => code).increment(1);
	failure(status, code)
}

/// An error and its sources on one line. Reqwest's own message only says that
/// a request failed; the reason is further down.
fn causes(error: &dyn std::error::Error) -> String {
	let mut text = error.to_string();
	let mut source = error.source();
	while let Some(next) = source {
		text.push_str(": ");
		text.push_str(&next.to_string());
		source = next.source();
	}
	text
}

/// What a request carried, taken back out of an error's text. Something too short
/// to hold a credential is left alone: an empty or one-letter path would
/// replace every matching letter and hide the very cause being logged.
fn without(text: String, carried: &str, placeholder: &str) -> String {
	const SHORTEST: usize = 4;
	if carried.len() < SHORTEST {
		return text;
	}
	text.replace(carried, placeholder)
}

/// External URLs can contain credentials; never keep them in a log line.
fn upstream_cause(error: reqwest::Error) -> String {
	causes(&error.without_url())
}

/// A request's name in logs. The token, path, query and header values stay out.
struct Label {
	service: String,
	method: String,
}

fn settle_failure(stage: &'static str, outcome: &'static str) {
	metrics::counter!("aidash_egress_settle_failures_total", "stage" => stage, "outcome" => outcome)
		.increment(1);
}

/// The time left to settle one request, and the wait between its attempts.
struct Settling {
	give_up: tokio::time::Instant,
	delay: Duration,
}
impl Settling {
	fn new() -> Self {
		Self {
			give_up: tokio::time::Instant::now() + SETTLE_BUDGET,
			delay: Duration::from_secs(1),
		}
	}

	/// Waits for the next attempt, or returns false when the time is up.
	async fn wait(&mut self) -> bool {
		let left = self
			.give_up
			.saturating_duration_since(tokio::time::Instant::now());
		if left.is_zero() {
			return false;
		}
		tokio::time::sleep(self.delay.min(left)).await;
		self.delay = (self.delay * 2).min(SETTLE_MAX_DELAY);
		true
	}
}

/// How settling one debit ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Settled {
	Charged,
	/// A decision. The bytes are committed (see [`Meter::charge`]).
	Refused(Refusal),
	/// The store stayed unavailable until the retry budget ran out, so the bytes
	/// are not charged.
	Abandoned,
}

/// Charges bytes that have already crossed the gateway. A refusal has committed
/// them (see [`Meter::charge`]) and is returned; anything else has not, so it is
/// tried again.
async fn charge_settling(
	meter: &dyn Meter,
	label: &Label,
	direction: Direction,
	bytes: u64,
	settling: &mut Settling,
) -> Settled {
	let bytes = bytes.min(usize::MAX as u64) as usize;
	loop {
		let cause = match tokio::time::timeout(SETTLE_ATTEMPT, meter.charge(direction, bytes)).await
		{
			Ok(Ok(())) => return Settled::Charged,
			Ok(Err(error)) => {
				let refusal = Refusal::of(&error);
				if !refusal.temporary() {
					return Settled::Refused(refusal);
				}
				error.to_string()
			}
			Err(_) => "attempt timed out".to_owned(),
		};
		if !settling.wait().await {
			settle_failure("debit", "abandoned");
			tracing::error!(
				service = %label.service,
				method = %label.method,
				bytes,
				%cause,
				"Egress debit abandoned; these bytes are not charged"
			);
			return Settled::Abandoned;
		}
		settle_failure("debit", "retried");
		tracing::debug!(service = %label.service, %cause, "Egress debit failed; retrying");
	}
}

async fn record_settling(
	meter: &dyn Meter,
	label: &Label,
	status: u16,
	times: RequestTimes,
	error: Option<&str>,
	settling: &mut Settling,
) {
	loop {
		let cause =
			match tokio::time::timeout(SETTLE_ATTEMPT, meter.finish(status, times, error)).await {
				Ok(Ok(())) => return,
				Ok(Err(error)) => error.to_string(),
				Err(_) => "attempt timed out".to_owned(),
			};
		if !settling.wait().await {
			settle_failure("record", "abandoned");
			tracing::error!(
				service = %label.service,
				method = %label.method,
				status,
				%cause,
				"Egress request record abandoned; it keeps status 0"
			);
			return;
		}
		settle_failure("record", "retried");
		tracing::debug!(service = %label.service, %cause, "Egress record failed; retrying");
	}
}

fn upstream_failed(label: &Label, cause: &str) {
	tracing::warn!(
		service = %label.service,
		method = %label.method,
		%cause,
		"Egress upstream failed"
	);
}

fn broker_failure(code: &str) -> (StatusCode, &'static str) {
	match code {
		"secret_unavailable" => (StatusCode::FAILED_DEPENDENCY, "secret_unavailable"),
		"secret_destination_refused" => (StatusCode::FORBIDDEN, "secret_destination_refused"),
		"rate_limited" => (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
		"upstream_unreachable" => (StatusCode::BAD_GATEWAY, "upstream_unreachable"),
		"broker_rejected" => (StatusCode::BAD_GATEWAY, "broker_rejected"),
		_ => (StatusCode::SERVICE_UNAVAILABLE, "broker_unavailable"),
	}
}
/// Whether a broker-owned error is a failure to reach the service, or of the
/// broker itself, rather than a decision about the Tenant's request.
fn broker_reports_failure(code: &str) -> bool {
	matches!(
		code,
		"upstream_unreachable" | "broker_rejected" | "broker_unavailable"
	)
}
/// The request's path under the service base's directory, appended to the broker's
/// `/v1/forward` base. The broker joins it to the signed prefix in turn.
fn broker_url(
	route: &aidash_application::tenant_secrets::SecretRoute,
	service: &Url,
	base: &Url,
) -> Result<Url, BoxError> {
	// The broker signs the base path's directory, so an endpoint-style base
	// such as `/invoke` forwards `invoke` exactly rather than `invoke/`.
	let prefix = aidash_application::tenant_secrets::path_directory(base.path());
	let relative = service
		.path()
		.strip_prefix(prefix)
		.ok_or_else(|| io_error("invalid_egress_path"))?;
	let mut target = route.base.as_str().trim_end_matches('/').to_owned();
	if !relative.is_empty() {
		target.push('/');
		target.push_str(relative);
	}
	if let Some(query) = service.query() {
		target.push('?');
		target.push_str(query);
	}
	Url::parse(&target).map_err(Into::into)
}
fn io_error(code: &'static str) -> BoxError {
	std::io::Error::other(code).into()
}

/// Why a debit stopped a stream. The request record and the client see the
/// same code, so an expired operation is never reported as an exhausted budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Refusal {
	status: StatusCode,
	code: &'static str,
}
impl Refusal {
	const LIMIT: Self = Self {
		status: StatusCode::TOO_MANY_REQUESTS,
		code: aidash_domain::egress::LIMIT_EXCEEDED,
	};
	const EXPIRED: Self = Self {
		status: StatusCode::FORBIDDEN,
		code: aidash_domain::egress::OPERATION_EXPIRED,
	};
	const INACTIVE: Self = Self {
		status: StatusCode::FORBIDDEN,
		code: aidash_domain::egress::OPERATION_INACTIVE,
	};
	const FORBIDDEN: Self = Self {
		status: StatusCode::FORBIDDEN,
		code: "egress_forbidden",
	};
	const UNAVAILABLE: Self = Self {
		status: StatusCode::SERVICE_UNAVAILABLE,
		code: aidash_domain::egress::UNAVAILABLE,
	};

	fn of(error: &aidash_application::Error) -> Self {
		use aidash_application::egress::DebitRefusal;
		match DebitRefusal::of(error) {
			Some(DebitRefusal::Limit) => Self::LIMIT,
			Some(DebitRefusal::Expired) => Self::EXPIRED,
			Some(DebitRefusal::Inactive) => Self::INACTIVE,
			Some(DebitRefusal::ScopeBudget) => Self {
				status: StatusCode::TOO_MANY_REQUESTS,
				code: "egress_budget_exhausted",
			},
			Some(DebitRefusal::Suspended) => Self {
				status: StatusCode::FORBIDDEN,
				code: "egress_suspended",
			},
			Some(DebitRefusal::Forbidden) => Self::FORBIDDEN,
			None => Self::UNAVAILABLE,
		}
	}

	fn response(self) -> Response<Body> {
		failure(self.status, self.code)
	}

	/// Not a decision about this request: the store could not be asked.
	fn temporary(self) -> bool {
		self.code == aidash_domain::egress::UNAVAILABLE
	}
}

/// Body bytes streamed but not yet debited, per direction. A flush takes the
/// whole pending count, so every byte is debited exactly once; dropping the
/// last handle debits whatever an abandoned stream left.
struct Debits {
	meter: Arc<dyn Meter>,
	label: Arc<Label>,
	sent: AtomicU64,
	received: AtomicU64,
	/// Bytes whose debit failed without committing. They are charged again only
	/// when the last handle drops, because the stream they came from has already
	/// failed and must not wait on the store a second time.
	failed_sent: AtomicU64,
	failed_received: AtomicU64,
	/// Every response body byte handed on so far, debited or not.
	handed: AtomicU64,
	/// The upload's refusal. Reqwest reports a failed request body only as a
	/// failed request, so the response path reads the real cause here.
	refused: OnceLock<Refusal>,
	/// Why the stream was cut off. The body stream owns [`Completion`], so the
	/// bridge that drops the stream on a stop sets this first; the record then
	/// names the stop instead of a client that went away. The first cause wins.
	interrupted: OnceLock<&'static str>,
}
impl Debits {
	fn new(meter: Arc<dyn Meter>, label: Label) -> Self {
		Self {
			meter,
			label: Arc::new(label),
			sent: AtomicU64::new(0),
			received: AtomicU64::new(0),
			failed_sent: AtomicU64::new(0),
			failed_received: AtomicU64::new(0),
			handed: AtomicU64::new(0),
			refused: OnceLock::new(),
			interrupted: OnceLock::new(),
		}
	}

	fn pending(&self, direction: Direction) -> &AtomicU64 {
		match direction {
			Direction::Sent => &self.sent,
			Direction::Received => &self.received,
		}
	}

	/// Takes every byte of `direction` that is still owed, including what a
	/// failed debit kept.
	fn take(&self, direction: Direction) -> u64 {
		let failed = match direction {
			Direction::Sent => &self.failed_sent,
			Direction::Received => &self.failed_received,
		};
		self.pending(direction).swap(0, Ordering::Relaxed) + failed.swap(0, Ordering::Relaxed)
	}

	fn keep_failed(&self, direction: Direction, bytes: u64) {
		match direction {
			Direction::Sent => &self.failed_sent,
			Direction::Received => &self.failed_received,
		}
		.fetch_add(bytes, Ordering::Relaxed);
	}

	/// Counts bytes before they are handed on, so a stream dropped while it
	/// waits to be polled again still debits them.
	fn count(&self, direction: Direction, bytes: usize) {
		self.pending(direction)
			.fetch_add(bytes as u64, Ordering::Relaxed);
		if matches!(direction, Direction::Received) {
			self.handed.fetch_add(bytes as u64, Ordering::Relaxed);
		}
	}

	/// Debits the pending bytes once a whole batch has accumulated.
	async fn flush_due(
		&self,
		direction: Direction,
		deadline: tokio::time::Instant,
	) -> Result<(), Refusal> {
		if self.pending(direction).load(Ordering::Relaxed) < BATCH_BYTES {
			return Ok(());
		}
		self.flush(direction, deadline).await
	}

	async fn flush(
		&self,
		direction: Direction,
		deadline: tokio::time::Instant,
	) -> Result<(), Refusal> {
		let bytes = self.pending(direction).swap(0, Ordering::Relaxed);
		if bytes == 0 {
			return Ok(());
		}
		let amount = usize::try_from(bytes).map_err(|_| Refusal::LIMIT)?;
		match tokio::time::timeout_at(deadline, self.meter.charge(direction, amount)).await {
			Ok(Ok(())) => Ok(()),
			Ok(Err(error)) => {
				let refusal = Refusal::of(&error);
				if refusal.temporary() {
					self.keep_failed(direction, bytes);
					settle_failure("debit", "retried");
					tracing::error!(
						service = %self.label.service,
						method = %self.label.method,
						cause = %error,
						"Egress debit failed; its bytes are kept to be charged again"
					);
				}
				Err(refusal)
			}
			Err(_) => {
				// The cancelled charge did not commit either.
				self.keep_failed(direction, bytes);
				Err(Refusal::EXPIRED)
			}
		}
	}
}
impl Drop for Debits {
	fn drop(&mut self) {
		let sent = *self.sent.get_mut() + *self.failed_sent.get_mut();
		let received = *self.received.get_mut() + *self.failed_received.get_mut();
		if sent == 0 && received == 0 {
			return;
		}
		let Ok(runtime) = tokio::runtime::Handle::try_current() else {
			return;
		};
		let meter = self.meter.clone();
		let label = self.label.clone();
		runtime.spawn(async move {
			let mut settling = Settling::new();
			for (direction, bytes) in [(Direction::Sent, sent), (Direction::Received, received)] {
				if bytes > 0 {
					charge_settling(&*meter, &label, direction, bytes, &mut settling).await;
				}
			}
		});
	}
}

/// Each stream, including a partially consumed one, debits its remaining
/// bytes and then completes its audit record.
struct Completion {
	debits: Arc<Debits>,
	started: Instant,
	deadline: tokio::time::Instant,
	status: u16,
	/// The `Content-Length` the client was promised, if any.
	expected: Option<u64>,
	finished: bool,
	admission_ms: u64,
	/// How long [`Completion::settle`] took, once it has run.
	settle_ms: u64,
	monitor: tokio::task::JoinHandle<()>,
}
async fn stopped(receiver: &mut watch::Receiver<Option<Refusal>>) -> Refusal {
	loop {
		if let Some(refusal) = *receiver.borrow_and_update() {
			return refusal;
		}
		if receiver.changed().await.is_err() {
			return Refusal::UNAVAILABLE;
		}
	}
}
impl Completion {
	async fn fail(
		&mut self,
		status: StatusCode,
		error: &'static str,
		message: &'static str,
	) -> Response<Body> {
		self.status = status.as_u16();
		self.finish(Some(error)).await;
		failure(status, message)
	}

	fn interrupted(&mut self, code: &'static str) {
		let _ = self.debits.interrupted.set(code);
	}
	/// Debits what both directions still hold. A stream that ended cleanly
	/// must do so within the operation deadline; a failed one gets a short
	/// grace so the bytes it did stream still count. A decision is returned in
	/// preference to a store that could not be asked.
	async fn settle(&mut self, error: Option<&str>) -> Option<Refusal> {
		let settling = tokio::time::Instant::now();
		let deadline = match error {
			None => self.deadline,
			Some(_) => self.deadline.max(tokio::time::Instant::now() + FLUSH_GRACE),
		};
		let sent = self.debits.flush(Direction::Sent, deadline).await;
		let received = self.debits.flush(Direction::Received, deadline).await;
		self.settle_ms = millis(settling.elapsed());
		let failures = [sent.err(), received.err()];
		failures
			.iter()
			.flatten()
			.find(|refusal| !refusal.temporary())
			.or(failures.iter().flatten().next())
			.copied()
	}

	/// Debits the remainder of a response that ended cleanly. Only a decision
	/// stops a response that is already on its way: when the store cannot be
	/// asked, the response ends and its bytes are charged, and its record
	/// written, in the background (#711).
	async fn settle_final(&mut self) -> Option<Refusal> {
		let ended = tokio::time::Instant::now();
		let duration_ms = millis(self.started.elapsed());
		match self.settle(None).await {
			Some(refusal) if refusal.temporary() => {
				self.settle_later(ended, duration_ms, true);
				None
			}
			refusal => refusal,
		}
	}

	/// Writes the request record in a task of its own, so that a client that
	/// disconnects cannot lose it. The response waits for the first attempt only
	/// briefly: the store may be stalled.
	async fn close(&mut self, error: Option<&str>) {
		if self.finished {
			return;
		}
		self.finished = true;
		let times = RequestTimes {
			duration_ms: millis(self.started.elapsed()),
			admission_ms: self.admission_ms,
			settle_ms: self.settle_ms,
		};
		let debits = self.debits.clone();
		let status = self.status;
		let error = error.map(str::to_owned);
		let writing = tokio::spawn(async move {
			let mut settling = Settling::new();
			record_settling(
				&*debits.meter,
				&debits.label,
				status,
				times,
				error.as_deref(),
				&mut settling,
			)
			.await;
		});
		let _ = tokio::time::timeout(RECORD_WAIT, writing).await;
	}

	/// Debits the remainder and completes the record. The stream's own failure
	/// stays the recorded outcome; a refused final debit is recorded, and
	/// returned, only for a stream that otherwise ended cleanly.
	async fn finish(&mut self, error: Option<&str>) -> Option<Refusal> {
		if self.finished {
			return None;
		}
		let refusal = match error {
			None => self.settle_final().await,
			Some(_) => self.settle(error).await,
		};
		if self.finished {
			// The final debit was left to the background.
			return None;
		}
		self.close(error.or(refusal.map(|refusal| refusal.code)))
			.await;
		refusal
	}

	/// Hands what is left to a task of its own: the debit, retried while the
	/// process lives, and then the record. `ended` is when the response ended
	/// and `complete` whether its body reached its end; hyper stops polling a
	/// body once it has written `Content-Length` bytes, so a complete response
	/// with a known length is dropped rather than finished. Complete means
	/// handed to Hyper: the client may still disconnect before it reads the
	/// rest, and a refused final debit can no longer stop it.
	fn settle_later(&mut self, ended: tokio::time::Instant, duration_ms: u64, complete: bool) {
		self.finished = true;
		let debits = self.debits.clone();
		let status = self.status;
		let admission_ms = self.admission_ms;
		let interrupted_error = self
			.debits
			.interrupted
			.get()
			.copied()
			.unwrap_or(STREAM_INTERRUPTED);
		tokio::spawn(async move {
			let mut settling = Settling::new();
			let mut refused = None;
			let mut abandoned = false;
			for direction in [Direction::Sent, Direction::Received] {
				let bytes = debits.take(direction);
				if bytes > 0 {
					match charge_settling(
						&*debits.meter,
						&debits.label,
						direction,
						bytes,
						&mut settling,
					)
					.await
					{
						Settled::Charged => {}
						Settled::Refused(refusal) => refused = refused.or(Some(refusal)),
						Settled::Abandoned => abandoned = true,
					}
				}
			}
			// A body that did not reach its end stays interrupted; otherwise a
			// decision wins over a debit that was given up.
			let error = if !complete {
				Some(interrupted_error)
			} else if let Some(refusal) = refused {
				Some(refusal.code)
			} else if abandoned {
				Some(DEBIT_ABANDONED)
			} else {
				None
			};
			let times = RequestTimes {
				duration_ms,
				admission_ms,
				settle_ms: millis(ended.elapsed()),
			};
			record_settling(
				&*debits.meter,
				&debits.label,
				status,
				times,
				error,
				&mut settling,
			)
			.await;
		});
	}
}
impl Drop for Completion {
	fn drop(&mut self) {
		self.monitor.abort();
		if !self.finished {
			let handed = self.debits.handed.load(Ordering::Relaxed);
			let complete = self.expected.is_some_and(|expected| handed >= expected);
			self.settle_later(
				tokio::time::Instant::now(),
				millis(self.started.elapsed()),
				complete,
			);
		}
	}
}

/// The listener must be bound only on a cluster-internal interface. Its token
/// is an operation credential, never a Tenant Secret or a Deployment secret.
/// Fails a socket write that makes no progress for the configured interval.
struct WriteDeadline<S> {
	inner: S,
	timeout: Duration,
	blocked: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
}
impl<S> WriteDeadline<S> {
	fn new(inner: S, timeout: Duration) -> Self {
		Self {
			inner,
			timeout,
			blocked: None,
		}
	}
	fn pending(
		&mut self,
		cx: &mut std::task::Context<'_>,
	) -> std::task::Poll<std::io::Result<usize>> {
		use std::future::Future;
		let timer = self
			.blocked
			.get_or_insert_with(|| Box::pin(tokio::time::sleep(self.timeout)));
		if timer.as_mut().poll(cx).is_ready() {
			std::task::Poll::Ready(Err(std::io::Error::new(
				std::io::ErrorKind::TimedOut,
				"egress_write_timeout",
			)))
		} else {
			std::task::Poll::Pending
		}
	}
}
impl<S: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for WriteDeadline<S> {
	fn poll_read(
		mut self: std::pin::Pin<&mut Self>,
		cx: &mut std::task::Context<'_>,
		buf: &mut tokio::io::ReadBuf<'_>,
	) -> std::task::Poll<std::io::Result<()>> {
		std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
	}
}
impl<S: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for WriteDeadline<S> {
	fn poll_write(
		mut self: std::pin::Pin<&mut Self>,
		cx: &mut std::task::Context<'_>,
		buf: &[u8],
	) -> std::task::Poll<std::io::Result<usize>> {
		match std::pin::Pin::new(&mut self.inner).poll_write(cx, buf) {
			std::task::Poll::Ready(result) => {
				self.blocked = None;
				std::task::Poll::Ready(result)
			}
			std::task::Poll::Pending => self.pending(cx),
		}
	}
	fn poll_flush(
		mut self: std::pin::Pin<&mut Self>,
		cx: &mut std::task::Context<'_>,
	) -> std::task::Poll<std::io::Result<()>> {
		match std::pin::Pin::new(&mut self.inner).poll_flush(cx) {
			std::task::Poll::Ready(result) => {
				self.blocked = None;
				std::task::Poll::Ready(result)
			}
			std::task::Poll::Pending => self.pending(cx).map(|result| result.map(|_| ())),
		}
	}
	fn poll_shutdown(
		mut self: std::pin::Pin<&mut Self>,
		cx: &mut std::task::Context<'_>,
	) -> std::task::Poll<std::io::Result<()>> {
		match std::pin::Pin::new(&mut self.inner).poll_shutdown(cx) {
			std::task::Poll::Ready(result) => {
				self.blocked = None;
				std::task::Poll::Ready(result)
			}
			std::task::Poll::Pending => self.pending(cx).map(|result| result.map(|_| ())),
		}
	}
}

/// Marks a connection whose request carries a body the gateway may leave unread.
/// `CONNECT` is excluded because an upgraded tunnel half-closes through the same
/// socket and must keep reading what the other side sends.
#[derive(Clone, Default)]
struct UnreadBody(
	Arc<std::sync::atomic::AtomicBool>,
	Arc<std::sync::Mutex<progress::Progress>>,
);
impl UnreadBody {
	fn note<B: hyper::body::Body>(&self, request: &Request<B>) {
		let has_body =
			request.method() != hyper::Method::CONNECT && !request.body().is_end_stream();
		self.0.store(has_body, Ordering::Relaxed);
		let mut progress = self.1.lock().unwrap();
		if request.method() == hyper::Method::CONNECT {
			progress.disable();
		} else {
			progress.note(request.body().size_hint().exact(), has_body);
		}
	}
	fn read(&self, bytes: &[u8]) {
		self.1.lock().unwrap().read(bytes);
	}
	fn incomplete(&self) -> bool {
		self.1.lock().unwrap().incomplete()
	}
}

/// Closes a connection as RFC 9112 section 9.6 asks. Closing a socket with unread
/// data resets it, which can destroy the answer on its way to the client, so the
/// sending side closes first and what the client still sends is discarded up to
/// a limit. A client past the limit may see a reset instead of the answer, and
/// the limit is counted. Whether the reason survives depends on the request: one
/// refused after admission is in its request record, while one refused before it
/// exists, or for want of a connection slot, is only counted.
struct LingeringClose<S> {
	inner: S,
	unread: UnreadBody,
	bytes_left: u64,
	linger: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
	closed: bool,
	/// Told once the answer has been written and the sending side is closed.
	answered: Option<tokio::sync::oneshot::Sender<()>>,
	tunnel: Option<Arc<std::sync::Mutex<Option<Arc<proxy::TunnelPermit>>>>>,
}
impl<S> LingeringClose<S> {
	fn new(inner: S, unread: UnreadBody) -> Self {
		Self {
			inner,
			unread,
			bytes_left: LINGER_BYTES,
			linger: None,
			closed: false,
			answered: None,
			tunnel: None,
		}
	}

	fn releasing_tunnel(
		mut self,
		tunnel: Arc<std::sync::Mutex<Option<Arc<proxy::TunnelPermit>>>>,
	) -> Self {
		self.tunnel = Some(tunnel);
		self
	}

	/// Reports through `answered` when the answer is out, so that a caller can
	/// bound the answering apart from the discarding that follows it.
	fn announcing(mut self, answered: tokio::sync::oneshot::Sender<()>) -> Self {
		self.answered = Some(answered);
		self
	}
}
impl<S: tokio::io::AsyncRead + Unpin> LingeringClose<S> {
	/// Discards until the client closes or a limit is reached.
	fn poll_discard(&mut self, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
		use std::future::Future;
		use std::task::Poll;
		let timer = self
			.linger
			.get_or_insert_with(|| Box::pin(tokio::time::sleep(LINGER_TIME)));
		if timer.as_mut().poll(cx).is_ready() {
			// Count an upload cut off while its body is still incomplete.
			if self.bytes_left < LINGER_BYTES && self.unread.incomplete() {
				metrics::counter!("aidash_egress_linger_limit_total", "limit" => "time")
					.increment(1);
			}
			return Poll::Ready(());
		}
		let mut scratch = [0; 16 * 1024];
		loop {
			let mut buffer = tokio::io::ReadBuf::new(&mut scratch);
			match std::pin::Pin::new(&mut self.inner).poll_read(cx, &mut buffer) {
				Poll::Pending => return Poll::Pending,
				Poll::Ready(Err(_)) => return Poll::Ready(()),
				Poll::Ready(Ok(())) if buffer.filled().is_empty() => return Poll::Ready(()),
				Poll::Ready(Ok(())) => {
					self.unread.read(buffer.filled());
					self.bytes_left = self.bytes_left.saturating_sub(buffer.filled().len() as u64);
					if self.bytes_left == 0 {
						metrics::counter!("aidash_egress_linger_limit_total", "limit" => "bytes")
							.increment(1);
						return Poll::Ready(());
					}
				}
			}
		}
	}
}
impl<S: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for LingeringClose<S> {
	fn poll_read(
		mut self: std::pin::Pin<&mut Self>,
		cx: &mut std::task::Context<'_>,
		buf: &mut tokio::io::ReadBuf<'_>,
	) -> std::task::Poll<std::io::Result<()>> {
		let before = buf.filled().len();
		let result = std::pin::Pin::new(&mut self.inner).poll_read(cx, buf);
		if matches!(result, std::task::Poll::Ready(Ok(()))) {
			self.unread.read(&buf.filled()[before..]);
		}
		result
	}
}
impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite
	for LingeringClose<S>
{
	fn poll_write(
		mut self: std::pin::Pin<&mut Self>,
		cx: &mut std::task::Context<'_>,
		buf: &[u8],
	) -> std::task::Poll<std::io::Result<usize>> {
		std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
	}
	fn poll_flush(
		mut self: std::pin::Pin<&mut Self>,
		cx: &mut std::task::Context<'_>,
	) -> std::task::Poll<std::io::Result<()>> {
		std::pin::Pin::new(&mut self.inner).poll_flush(cx)
	}
	fn poll_shutdown(
		mut self: std::pin::Pin<&mut Self>,
		cx: &mut std::task::Context<'_>,
	) -> std::task::Poll<std::io::Result<()>> {
		if !self.closed {
			std::task::ready!(std::pin::Pin::new(&mut self.inner).poll_shutdown(cx))?;
			self.closed = true;
			if let Some(answered) = self.answered.take() {
				let _ = answered.send(());
			}
		}
		if self.unread.0.load(Ordering::Relaxed) {
			if let Some(tunnel) = self.tunnel.take() {
				drop(tunnel.lock().unwrap().take());
			}
			std::task::ready!(self.poll_discard(cx));
		}
		std::task::Poll::Ready(Ok(()))
	}
}

pub async fn serve(
	listener: TcpListener,
	authority: Arc<dyn Authority>,
	stopping: watch::Receiver<bool>,
) -> crate::Result<()> {
	serve_with_scopes(listener, authority, None, ProxyLimits::default(), stopping).await
}

/// Per-instance proxy capacity. Tenant capacity must be lower than global capacity.
#[derive(Clone, Copy, Debug)]
pub struct ProxyLimits {
	pub global_concurrent: usize,
	pub tenant_concurrent: usize,
}
impl Default for ProxyLimits {
	fn default() -> Self {
		Self {
			global_concurrent: 1024,
			tenant_concurrent: 32,
		}
	}
}

/// Serve legacy External service calls and, when configured, scope proxy calls.
pub async fn serve_with_scopes(
	listener: TcpListener,
	authority: Arc<dyn Authority>,
	scopes: Option<Arc<dyn aidash_application::egress::EgressScopes>>,
	limits: ProxyLimits,
	stopping: watch::Receiver<bool>,
) -> crate::Result<()> {
	if limits.tenant_concurrent == 0
		|| limits.tenant_concurrent >= limits.global_concurrent
		|| limits.global_concurrent > tokio::sync::Semaphore::MAX_PERMITS
	{
		return Err(aidash_application::Error::Invalid(
			"invalid Egress proxy tunnel capacity".into(),
		));
	}
	let client = reqwest::Client::builder()
		.redirect(reqwest::redirect::Policy::none())
		.no_proxy()
		.connect_timeout(IDLE)
		.read_timeout(IDLE)
		.build()
		.map_err(crate::http_error)?;
	let public = reqwest::Client::builder()
		.redirect(reqwest::redirect::Policy::none())
		.no_proxy()
		.dns_resolver(Arc::new(crate::keyless::PublicDns))
		.connect_timeout(IDLE)
		.read_timeout(IDLE)
		.build()
		.map_err(crate::http_error)?;
	serve_with_clients(
		listener, authority, scopes, limits, stopping, client, public,
	)
	.await
}

/// The serving loop, with the upstream clients supplied. Tests give it clients
/// that reach a local stand-in for the package registries.
async fn serve_with_clients(
	listener: TcpListener,
	authority: Arc<dyn Authority>,
	scopes: Option<Arc<dyn aidash_application::egress::EgressScopes>>,
	limits: ProxyLimits,
	mut stopping: watch::Receiver<bool>,
	client: reqwest::Client,
	public: reqwest::Client,
) -> crate::Result<()> {
	let slots = Arc::new(tokio::sync::Semaphore::new(CONNECTION_SLOTS));
	let refusals = Arc::new(tokio::sync::Semaphore::new(REFUSAL_SLOTS));
	let mut refused = RefusedConnections::default();
	let mut connections = tokio::task::JoinSet::new();
	let tunnels = Arc::new(proxy::TunnelManager::new(stopping.clone(), limits));
	let mut failures = crate::accept::AcceptFailures::new("egress gateway");
	loop {
		if *stopping.borrow() || stopping.has_changed().is_err() {
			connections.abort_all();
			while connections.join_next().await.is_some() {}
			tunnels.drain().await;
			return Ok(());
		}
		tokio::select! {
			_ = stopping.changed() => {},
			Some(_) = connections.join_next(), if !connections.is_empty() => {},
			accepted = listener.accept() => {
				let Some((socket, _)) = failures.settle(accepted, &mut stopping).await else { continue; };
				let Ok(permit) = slots.clone().try_acquire_owned() else {
					let answering = refusals.clone().try_acquire_owned();
					refused.note(answering.is_ok());
					if let Ok(permit) = answering {
						connections.spawn(async move { let _permit = permit; refuse_connection(socket).await; });
					}
					continue;
				};
				let authority = authority.clone(); let client = client.clone(); let public = public.clone(); let scopes = scopes.clone(); let tunnels = tunnels.clone();
				connections.spawn(async move {
					let permit = Arc::new(std::sync::Mutex::new(Some(permit)));
					let connection_lease = Arc::new(std::sync::Mutex::new(None));
					let unread = UnreadBody::default();
					let noting = unread.clone();
					let lingering_tunnel = connection_lease.clone();
					let service = service_fn(move |request| {
						noting.note(&request);
						let authority=authority.clone(); let client=client.clone(); let public=public.clone(); let scopes=scopes.clone(); let permit=permit.clone(); let tunnels=tunnels.clone(); let connection_lease=connection_lease.clone();
						async move {
							let response = if request.method() == hyper::Method::CONNECT || request.uri().scheme().is_some() {
								let connect = request.method() == hyper::Method::CONNECT;
								let mut response = match scopes { Some(scopes) => proxy::forward(request, scopes, tunnels, permit, connection_lease).await, None => refuse(StatusCode::SERVICE_UNAVAILABLE, "egress_scope_unavailable") };
								if !connect || response.status() != StatusCode::OK { response.headers_mut().insert(hyper::header::CONNECTION, hyper::header::HeaderValue::from_static("close")); }
								response
							} else { forward(request, authority, client, public, scopes).await };
							Ok::<_,Infallible>(response)
						}
					});
					let _ = hyper::server::conn::http1::Builder::new().timer(TokioTimer::new())
						.header_read_timeout(IDLE).max_headers(64)
						.serve_connection(TokioIo::new(LingeringClose::new(WriteDeadline::new(socket, IDLE), unread).releasing_tunnel(lingering_tunnel)), service).with_upgrades().await;
				});
			}
		}
	}
}

/// Connections refused since the last log line.
#[derive(Default)]
struct RefusedConnections {
	unlogged: u64,
	logged_at: Option<Instant>,
}
impl RefusedConnections {
	fn note(&mut self, answered: bool) {
		let outcome = if answered { "answered" } else { "dropped" };
		metrics::counter!("aidash_egress_connections_refused_total", "outcome" => outcome)
			.increment(1);
		self.unlogged += 1;
		if self
			.logged_at
			.is_none_or(|at| at.elapsed() >= REFUSAL_LOG_INTERVAL)
		{
			tracing::warn!(
				refused = self.unlogged,
				slots = CONNECTION_SLOTS,
				"Egress connection slots are full"
			);
			self.unlogged = 0;
			self.logged_at = Some(Instant::now());
		}
	}
}

/// Reads the request head first: closing a socket with unread data resets it,
/// which can destroy the answer on its way to the client. Only the answering is
/// bounded by `REFUSAL_WINDOW`; what the client still sends afterwards is
/// discarded under the wrapper's own limits (#726), as for any refusal.
async fn refuse_connection(socket: tokio::net::TcpStream) {
	let unread = UnreadBody::default();
	let noting = unread.clone();
	let service = service_fn(move |request| {
		noting.note(&request);
		async {
			let mut response = failure(StatusCode::SERVICE_UNAVAILABLE, CONNECTION_LIMIT);
			response.headers_mut().insert(
				hyper::header::CONNECTION,
				hyper::header::HeaderValue::from_static("close"),
			);
			Ok::<_, Infallible>(response)
		}
	});
	let (answered, answering) = tokio::sync::oneshot::channel();
	let serving = hyper::server::conn::http1::Builder::new()
		.timer(TokioTimer::new())
		.keep_alive(false)
		.header_read_timeout(REFUSAL_WINDOW)
		.max_headers(64)
		.serve_connection(
			TokioIo::new(LingeringClose::new(socket, unread).announcing(answered)),
			service,
		);
	tokio::pin!(serving);
	tokio::select! {
		_ = &mut serving => return,
		answered = tokio::time::timeout(REFUSAL_WINDOW, answering) => {
			if answered.is_err() {
				return;
			}
		}
	}
	let _ = serving.await;
}

fn target(admission: &Admission, path: &str, query: Option<&str>) -> Result<Url, BoxError> {
	let base = Url::parse(&admission.definition.base_url)?;
	let mut url = base.join(path.trim_start_matches('/'))?;
	if url.origin() != base.origin()
		|| !url.path().starts_with(base.path())
		|| !url.username().is_empty()
		|| url.password().is_some()
	{
		return Err(io_error("invalid_egress_path"));
	}
	url.set_query(query);
	url.set_fragment(None);
	Ok(url)
}

fn forwarded_header(name: &str, connection: &[String]) -> bool {
	!matches!(
		name,
		"authorization"
			| "proxy-authorization"
			| "proxy-authenticate"
			| "cookie"
			| "set-cookie"
			| "host" | "connection"
			| "keep-alive"
			| "transfer-encoding"
			| "te" | "trailer"
			| "upgrade"
			| "expect"
			| "origin"
			| "referer"
	) && !name.starts_with("x-forwarded-")
		&& !connection.iter().any(|v| v == name)
}

/// A rewritten package index must reach the gateway uncompressed: the client's
/// own `accept-encoding` would sit beside the gateway's `identity` and let the
/// index answer with gzip, which the JSON/HTML rewrite cannot read (#679).
fn request_header_forwarded(name: &str, connection: &[String], rewrites_index: bool) -> bool {
	forwarded_header(name, connection) && !(rewrites_index && name == "accept-encoding")
}

fn package_url(service: &str, url: &Url) -> bool {
	url.scheme() == "https"
		&& url.username().is_empty()
		&& url.password().is_none()
		&& url.port_or_known_default() == Some(443)
		&& matches!(
			(service, url.host_str()),
			("core.npm", Some("registry.npmjs.org"))
				| ("core.pypi", Some("pypi.org" | "files.pythonhosted.org"))
		)
}
fn package_link(
	value: &str,
	source: &Url,
	origin: &str,
	token: &str,
	service: &str,
) -> Option<String> {
	let url = source.join(value).ok()?;
	if !package_url(service, &url) {
		return None;
	}
	let mut url = url;
	let fragment = url.fragment().map(str::to_owned);
	url.set_fragment(None);
	Some(format!(
		"{}/v1/{token}/{service}/__asset/{}{}",
		origin.trim_end_matches('/'),
		URL_SAFE_NO_PAD.encode(url.as_str()),
		fragment.map(|f| format!("#{f}")).unwrap_or_default()
	))
}
fn rewrite_json(
	bytes: &[u8],
	source: &Url,
	origin: &str,
	token: &str,
	service: &str,
) -> Result<Vec<u8>, BoxError> {
	fn visit(
		value: &mut serde_json::Value,
		source: &Url,
		origin: &str,
		token: &str,
		service: &str,
	) {
		match value {
			serde_json::Value::Array(items) => {
				for item in items {
					visit(item, source, origin, token, service);
				}
			}
			serde_json::Value::Object(items) => {
				for (key, value) in items {
					if matches!(key.as_str(), "tarball" | "url" | "href")
						&& let Some(link) = value
							.as_str()
							.and_then(|v| package_link(v, source, origin, token, service))
					{
						*value = link.into();
					} else {
						visit(value, source, origin, token, service);
					}
				}
			}
			_ => {}
		}
	}
	let mut value = serde_json::from_slice(bytes)?;
	visit(&mut value, source, origin, token, service);
	Ok(serde_json::to_vec(&value)?)
}
fn rewrite_html(
	bytes: &[u8],
	source: &Url,
	origin: &str,
	token: &str,
	service: &str,
) -> Result<Vec<u8>, BoxError> {
	let text = std::str::from_utf8(bytes)?;
	let mut result = String::with_capacity(text.len());
	let mut rest = text;
	// PyPI's Simple API uses quoted href attributes. Unknown or foreign links
	// remain unreachable under the Pod's NetworkPolicy.
	while let Some(start) = rest.find("href=") {
		result.push_str(&rest[..start + 5]);
		rest = &rest[start + 5..];
		let Some(quote) = rest.chars().next().filter(|c| matches!(c, '\'' | '"')) else {
			continue;
		};
		result.push(quote);
		rest = &rest[quote.len_utf8()..];
		let Some(end) = rest.find(quote) else {
			return Err(io_error("invalid_package_index"));
		};
		let value = &rest[..end];
		let decoded = value.replace("&amp;", "&");
		result.push_str(
			&package_link(&decoded, source, origin, token, service)
				.unwrap_or_else(|| value.to_owned()),
		);
		result.push(quote);
		rest = &rest[end + quote.len_utf8()..];
	}
	result.push_str(rest);
	Ok(result.into_bytes())
}

async fn forward(
	request: Request<hyper::body::Incoming>,
	authority: Arc<dyn Authority>,
	client: reqwest::Client,
	public: reqwest::Client,
	scopes: Option<Arc<dyn aidash_application::egress::EgressScopes>>,
) -> Response<Body> {
	if request.uri().scheme().is_some() {
		return refuse(StatusCode::NOT_FOUND, "egress_route_not_found");
	}
	let request_path = request.uri().path();
	let scoped = request_path.starts_with("/v2/services/");
	let route = if scoped {
		request_path[13..].to_owned()
	} else if let Some(path) = request_path.strip_prefix("/v1/") {
		path.to_owned()
	} else {
		return refuse(StatusCode::NOT_FOUND, "egress_route_not_found");
	};
	let scope_token = if scoped {
		let mut values = request
			.headers()
			.get_all(hyper::header::AUTHORIZATION)
			.iter();
		let token = values
			.next()
			.and_then(|v| v.to_str().ok())
			.and_then(|s| s.split_once(' '))
			.filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
			.map(|(_, token)| token)
			.filter(|s| !s.is_empty());
		if values.next().is_some() || token.is_none() {
			return refuse(StatusCode::UNAUTHORIZED, "egress_token_required");
		}
		token.map(str::to_owned)
	} else {
		None
	};
	let (token, remainder) = if let Some(token) = scope_token.as_deref() {
		(token, route.as_str())
	} else {
		let Some((token, remainder)) = route.split_once('/') else {
			return refuse(StatusCode::UNAUTHORIZED, "egress_token_required");
		};
		(token, remainder)
	};
	let (service, path) = remainder.split_once('/').unwrap_or((remainder, ""));
	if token.len() > 512
		|| service.is_empty()
		|| service.len() > 100
		|| !service
			.bytes()
			.all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
	{
		return refuse(StatusCode::BAD_REQUEST, "invalid_egress_route");
	}
	let asset = if let Some(encoded) = path.strip_prefix("__asset/") {
		if scoped {
			return refuse(StatusCode::BAD_REQUEST, "invalid_egress_path");
		}
		let decoded = URL_SAFE_NO_PAD
			.decode(encoded)
			.ok()
			.and_then(|b| String::from_utf8(b).ok())
			.and_then(|s| Url::parse(&s).ok());
		let Some(url) = decoded.filter(|u| package_url(service, u)) else {
			return refuse(StatusCode::BAD_REQUEST, "invalid_package_url");
		};
		Some(url)
	} else {
		None
	};
	let query = asset
		.as_ref()
		.and_then(Url::query)
		.or_else(|| request.uri().query())
		.map(str::to_owned);
	let audit_path = asset.as_ref().map(Url::path).unwrap_or(path);
	let head = request.method() == hyper::Method::HEAD;
	let query_digest = format!(
		"{:x}",
		Sha256::digest(query.as_deref().unwrap_or("").as_bytes())
	);
	let admitting = tokio::time::Instant::now();
	let admitted = if scoped {
		match scopes {
			Some(scopes) => {
				scopes
					.admit_external(
						token,
						service,
						request.method().as_str(),
						audit_path,
						&query_digest,
					)
					.await
			}
			None => Err(aidash_application::Error::Forbidden),
		}
	} else {
		authority
			.admit(
				token,
				service,
				request.method().as_str(),
				audit_path,
				&query_digest,
			)
			.await
	};
	let admission_ms = millis(admitting.elapsed());
	let admission = match admitted {
		Ok(value) => value,
		Err(aidash_application::Error::Forbidden | aidash_application::Error::Unauthorized) => {
			return refuse(StatusCode::FORBIDDEN, "egress_forbidden");
		}
		Err(aidash_application::Error::Conflict(code))
			if code == aidash_domain::egress::OPERATION_EXPIRED =>
		{
			return refuse(Refusal::EXPIRED.status, Refusal::EXPIRED.code);
		}
		Err(aidash_application::Error::Conflict(code))
			if code == aidash_domain::egress::OPERATION_INACTIVE =>
		{
			return refuse(Refusal::INACTIVE.status, Refusal::INACTIVE.code);
		}
		Err(aidash_application::Error::Conflict(_)) => {
			return refuse(Refusal::LIMIT.status, Refusal::LIMIT.code);
		}
		Err(aidash_application::Error::Refused(message))
			if message == "egress_budget_exhausted" =>
		{
			return refuse(StatusCode::TOO_MANY_REQUESTS, "egress_budget_exhausted");
		}
		Err(aidash_application::Error::Refused(message)) if message == "egress_suspended" => {
			return refuse(StatusCode::FORBIDDEN, "egress_suspended");
		}
		Err(aidash_application::Error::Invalid(message)) if message == "secret_unavailable" => {
			return refuse(StatusCode::FAILED_DEPENDENCY, "secret_unavailable");
		}
		// The Sandbox sees only the code; the cause stays in the server's log. Where
		// an error's text came from is not under this function's control, so what
		// the request itself carried is taken back out.
		Err(error) => {
			let cause = without(
				without(error.to_string(), token, "[token]"),
				audit_path,
				"[path]",
			);
			tracing::error!(
				service,
				method = %request.method(),
				%cause,
				"Egress admission failed"
			);
			return refuse(
				StatusCode::SERVICE_UNAVAILABLE,
				aidash_domain::egress::UNAVAILABLE,
			);
		}
	};
	let remaining = (admission.expires_at - chrono::Utc::now())
		.to_std()
		.unwrap_or_default();
	let deadline = tokio::time::Instant::now() + remaining;
	let (active_tx, mut active_rx) = watch::channel(None);
	let meter = admission.meter.clone();
	let monitor = tokio::spawn(async move {
		let mut interval = tokio::time::interval(Duration::from_secs(4));
		interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
		interval.tick().await;
		let mut failures = 0;
		loop {
			interval.tick().await;
			match tokio::time::timeout(Duration::from_secs(1), meter.check_active()).await {
				Ok(Ok(())) => failures = 0,
				Ok(Err(error)) if Refusal::of(&error) != Refusal::UNAVAILABLE => {
					let _ = active_tx.send(Some(Refusal::of(&error)));
					break;
				}
				_ => {
					failures += 1;
					if failures >= 3 {
						let _ = active_tx.send(Some(Refusal::UNAVAILABLE));
						break;
					}
				}
			}
		}
	});
	let mut completion = Completion {
		debits: Arc::new(Debits::new(
			admission.meter.clone(),
			Label {
				service: service.to_owned(),
				method: request.method().to_string(),
			},
		)),
		started: Instant::now(),
		deadline,
		status: 502,
		expected: None,
		finished: false,
		monitor,
		admission_ms,
		settle_ms: 0,
	};
	if remaining.is_zero() {
		return completion
			.fail(StatusCode::FORBIDDEN, "operation_expired", "egress_expired")
			.await;
	}
	let index = !scoped && admission.definition.builtin_index(service);
	if asset.is_some() && !index {
		return completion
			.fail(
				StatusCode::FORBIDDEN,
				"invalid_package_service",
				"egress_forbidden",
			)
			.await;
	}
	let service_url = match asset
		.clone()
		.map(Ok)
		.unwrap_or_else(|| target(&admission, path, query.as_deref()))
	{
		Ok(url) => url,
		Err(_) => {
			return completion
				.fail(
					StatusCode::BAD_REQUEST,
					"invalid_path",
					"invalid_egress_path",
				)
				.await;
		}
	};
	// A brokered request goes to the broker's `/v1/forward` base instead of the
	// External service. The broker attaches the Tenant Secret (ADR 0095).
	let brokered = admission.route.is_some();
	let upstream_url = match &admission.route {
		Some(route) => {
			let base = Url::parse(&admission.definition.base_url).expect("admitted service URL");
			match broker_url(route, &service_url, &base) {
				Ok(url) => url,
				Err(_) => {
					return completion
						.fail(
							StatusCode::BAD_REQUEST,
							"invalid_path",
							"invalid_egress_path",
						)
						.await;
				}
			}
		}
		None => service_url.clone(),
	};
	let connection = request
		.headers()
		.get("connection")
		.and_then(|v| v.to_str().ok())
		.unwrap_or("")
		.split(',')
		.map(|s| s.trim().to_ascii_lowercase())
		.collect::<Vec<_>>();
	// The broker is an in-cluster service: it resolves the External service
	// itself and applies the public-address rule there.
	let selected = if brokered {
		&client
	} else {
		match admission.destination {
			aidash_domain::keyless::Destination::PublicOnly => &public,
			aidash_domain::keyless::Destination::Permitted => &client,
			aidash_domain::keyless::Destination::Forbidden => {
				return completion
					.fail(
						StatusCode::FORBIDDEN,
						"destination_forbidden",
						"egress_forbidden",
					)
					.await;
			}
		}
	};
	let mut upstream = selected.request(request.method().clone(), upstream_url);
	let rewrites_index = index && asset.is_none();
	for (name, value) in request.headers() {
		if request_header_forwarded(name.as_str(), &connection, rewrites_index)
			&& name.as_str()
				!= admission
					.definition
					.authentication
					.header()
					.to_ascii_lowercase()
			// The broker owns this header; an inbound one must never override
			// the token the gateway mints.
			&& !(brokered && name.as_str() == SECRET_CALL)
		{
			upstream = upstream.header(name, value);
		}
	}
	if let Some(route) = &admission.route {
		let Ok(mut value) = hyper::header::HeaderValue::from_str(route.token.expose_secret())
		else {
			return completion
				.fail(
					StatusCode::SERVICE_UNAVAILABLE,
					"broker_unavailable",
					"broker_unavailable",
				)
				.await;
		};
		value.set_sensitive(true);
		upstream = upstream.header(SECRET_CALL, value);
	} else if let Some(secret) = &admission.credential {
		upstream = match &admission.definition.authentication {
			aidash_domain::egress::Authentication::Bearer => {
				upstream.bearer_auth(secret.expose_secret())
			}
			aidash_domain::egress::Authentication::Header { name } => {
				let Ok(mut value) = hyper::header::HeaderValue::from_str(secret.expose_secret())
				else {
					return completion
						.fail(
							StatusCode::FAILED_DEPENDENCY,
							"secret_unavailable",
							"secret_unavailable",
						)
						.await;
				};
				value.set_sensitive(true);
				upstream.header(name, value)
			}
		};
	}
	if rewrites_index {
		upstream = upstream.header("accept-encoding", "identity");
	}
	let debits = completion.debits.clone();
	let mut incoming = request.into_body().into_data_stream();
	let body = async_stream::try_stream! {
		loop {
			let frame = tokio::time::timeout_at(deadline, tokio::time::timeout(IDLE,incoming.next())).await
				.map_err(|_|io_error("operation_expired"))?.map_err(|_|io_error("egress_idle_timeout"))?;
			let Some(frame) = frame else { break; };
			let bytes = frame.map_err(|_|io_error("egress_upload_interrupted"))?;
			debits.count(Direction::Sent, bytes.len());
			yield bytes;
			if let Err(refusal) = debits.flush_due(Direction::Sent, deadline).await {
				let _ = debits.refused.set(refusal);
				Err(io_error(refusal.code))?;
			}
		}
		// The whole upload is debited before the request is complete.
		if let Err(refusal) = debits.flush(Direction::Sent, deadline).await {
			let _ = debits.refused.set(refusal);
			Err(io_error(refusal.code))?;
		}
	};
	let body = body.map_err(|error: BoxError| error);
	let response = match tokio::select! {
		refusal = stopped(&mut active_rx) => {
			completion.status = refusal.status.as_u16();
			completion.interrupted(refusal.code);
			return refusal.response();
		}
		response = tokio::time::timeout_at(
		deadline,
		upstream.body(reqwest::Body::wrap_stream(body)).send(),
		) => response,
	} {
		Ok(Ok(response)) => response,
		failed => {
			// A refused upload debit fails the request; report the refusal
			// rather than an unreachable upstream.
			if let Some(refusal) = completion.debits.refused.get().copied() {
				completion.status = refusal.status.as_u16();
				completion.finish(Some(refusal.code)).await;
				return refusal.response();
			}
			let cause = match failed {
				Ok(Err(error)) => upstream_cause(error),
				_ => "the operation deadline passed".to_owned(),
			};
			upstream_failed(&completion.debits.label, &cause);
			return completion
				.fail(
					StatusCode::BAD_GATEWAY,
					"upstream_unavailable",
					"EGRESS_UNAVAILABLE",
				)
				.await;
		}
	};
	completion.status = response.status().as_u16();
	// A broker-owned error is returned as a gateway error that names the code
	// only; an upstream status the broker passes through stays as it is.
	if brokered
		&& let Some(code) = response
			.headers()
			.get(BROKER_ERROR)
			.and_then(|value| value.to_str().ok())
	{
		let (status, code) = broker_failure(code);
		if broker_reports_failure(code) {
			upstream_failed(&completion.debits.label, code);
		}
		completion.status = status.as_u16();
		completion.finish(Some(code)).await;
		return failure(status, code);
	}
	let mut headers = response.headers().clone();
	let connection = headers
		.get("connection")
		.and_then(|v| v.to_str().ok())
		.unwrap_or("")
		.split(',')
		.map(|s| s.trim().to_ascii_lowercase())
		.collect::<Vec<_>>();
	for name in headers.keys().cloned().collect::<Vec<_>>() {
		if !forwarded_header(name.as_str(), &connection)
			|| name.as_str()
				== admission
					.definition
					.authentication
					.header()
					.to_ascii_lowercase()
		{
			headers.remove(name);
		}
	}
	// A redirect is followed through the gateway, with a fresh live decision and
	// another request debit. Streaming uploads are never buffered for replay.
	if response.status().is_redirection()
		&& let Some(location) = headers.get("location")
	{
		let redirected = location
			.to_str()
			.ok()
			.and_then(|s| service_url.join(s).ok());
		let base = Url::parse(&admission.definition.base_url).expect("admitted service URL");
		let Some(redirected) = redirected.filter(|u| {
			u.origin() == base.origin()
				&& u.path().starts_with(base.path())
				&& u.username().is_empty()
				&& u.password().is_none()
		}) else {
			return completion
				.fail(
					StatusCode::BAD_GATEWAY,
					"redirect_refused",
					"egress_redirect_refused",
				)
				.await;
		};
		let relative = redirected.path().strip_prefix(base.path()).unwrap_or("");
		let prefix = if scoped {
			format!("/v2/services/{service}/{relative}")
		} else {
			format!("/v1/{token}/{service}/{relative}")
		};
		let rewritten = format!(
			"{prefix}{}",
			redirected
				.query()
				.map(|q| format!("?{q}"))
				.unwrap_or_default()
		);
		let Ok(location) = rewritten.parse() else {
			return completion
				.fail(
					StatusCode::BAD_GATEWAY,
					"redirect_refused",
					"egress_redirect_refused",
				)
				.await;
		};
		headers.insert("location", location);
	}
	headers.insert("cache-control", "no-store".parse().unwrap());
	let status = response.status();
	// Hyper does not poll bodies for these responses. Complete their audit now,
	// rather than classifying a correctly empty response as an interruption.
	if head || matches!(status, StatusCode::NO_CONTENT | StatusCode::NOT_MODIFIED) {
		if let Some(refusal) = completion.settle_final().await {
			completion.status = refusal.status.as_u16();
			completion.close(Some(refusal.code)).await;
			return refusal.response();
		}
		completion.close(None).await;
		let mut result = Response::new(
			Full::new(Bytes::new())
				.map_err(|never| match never {})
				.boxed_unsync(),
		);
		*result.status_mut() = status;
		*result.headers_mut() = headers;
		return result;
	}
	let index_type = response
		.headers()
		.get("content-type")
		.and_then(|v| v.to_str().ok())
		.unwrap_or("")
		.to_owned();
	let mut downstream = response.bytes_stream();
	if index
		&& asset.is_none()
		&& !head
		&& (index_type.contains("json") || index_type.starts_with("text/html"))
	{
		let mut buffer = Vec::new();
		loop {
			let next = tokio::select! {
				refusal = stopped(&mut active_rx) => {
					completion.status = refusal.status.as_u16();
					completion.interrupted(refusal.code);
					return refusal.response();
				}
				next = tokio::time::timeout_at(deadline, tokio::time::timeout(IDLE, downstream.next())) => next,
			};
			let bytes = match next {
				Ok(Ok(Some(Ok(bytes)))) => bytes,
				Ok(Ok(None)) => break,
				failed => {
					let cause = match failed {
						Ok(Ok(Some(Err(error)))) => upstream_cause(error),
						_ => "timed out".to_owned(),
					};
					upstream_failed(&completion.debits.label, &cause);
					return completion
						.fail(
							StatusCode::BAD_GATEWAY,
							"index_unavailable",
							"package_index_unavailable",
						)
						.await;
				}
			};
			completion.debits.count(Direction::Received, bytes.len());
			if let Err(refusal) = completion
				.debits
				.flush_due(Direction::Received, deadline)
				.await
			{
				completion.status = refusal.status.as_u16();
				completion.finish(Some(refusal.code)).await;
				return refusal.response();
			}
			if buffer.len().saturating_add(bytes.len()) > INDEX_BYTES {
				let mut response = completion
					.fail(
						StatusCode::UNPROCESSABLE_ENTITY,
						"index_limit_exceeded",
						INDEX_LIMIT_MESSAGE,
					)
					.await;
				response.extensions_mut().insert(
					hyper::ext::ReasonPhrase::try_from(INDEX_LIMIT_MESSAGE.as_bytes())
						.expect("valid reason phrase"),
				);
				return response;
			}
			buffer.extend_from_slice(&bytes);
		}
		let rewritten = if index_type.contains("json") {
			rewrite_json(
				&buffer,
				&service_url,
				&admission.gateway_origin,
				token,
				service,
			)
		} else {
			rewrite_html(
				&buffer,
				&service_url,
				&admission.gateway_origin,
				token,
				service,
			)
		};
		let Ok(rewritten) = rewritten else {
			return completion
				.fail(
					StatusCode::BAD_GATEWAY,
					"invalid_package_index",
					"invalid_package_index",
				)
				.await;
		};
		// Rewritten links can lengthen the index the sandbox receives.
		completion.debits.count(
			Direction::Received,
			rewritten.len().saturating_sub(buffer.len()),
		);
		if let Some(refusal) = completion.settle_final().await {
			completion.status = refusal.status.as_u16();
			completion.close(Some(refusal.code)).await;
			return refusal.response();
		}
		completion.close(None).await;
		headers.remove("content-length");
		headers.remove("content-encoding");
		headers.remove("etag");
		headers.remove("content-md5");
		let mut result = Response::new(
			Full::new(Bytes::from(rewritten))
				.map_err(|never| match never {})
				.boxed_unsync(),
		);
		*result.status_mut() = status;
		*result.headers_mut() = headers;
		return result;
	}
	completion.expected = headers
		.get(hyper::header::CONTENT_LENGTH)
		.and_then(|value| value.to_str().ok())
		.and_then(|value| value.parse().ok());
	let debits = completion.debits.clone();
	let bridge_debits = completion.debits.clone();
	let mut bridge_active = active_rx.clone();
	let output = async_stream::try_stream! {
		loop {
			let checked = tokio::select! {
				refusal = stopped(&mut active_rx) => Err(refusal),
				next = tokio::time::timeout_at(deadline,tokio::time::timeout(IDLE,downstream.next())) => Ok(next),
			};
			let next = match checked {
				Ok(next) => next,
				Err(refusal) => { completion.interrupted(refusal.code); Err(io_error(refusal.code))?; unreachable!() }
			};
			let next = match next {
				Ok(Ok(next)) => next,
				_ => { upstream_failed(&debits.label, "timed out"); completion.finish(Some("egress_timeout")).await; Err(io_error("egress_timeout"))?; unreachable!() }
			};
			let Some(bytes) = next else {
				// The final debit can still refuse; the client then sees an
				// incomplete body rather than a clean end.
				if let Some(refusal) = completion.finish(None).await { Err(io_error(refusal.code))?; }
				break;
			};
			let bytes = match bytes {
				Ok(bytes) => bytes,
				Err(error) => {
					// An upstream that echoes the upload fails when the upload's
					// debit was refused; that refusal is the real cause.
					if debits.refused.get().is_none() { upstream_failed(&debits.label, &upstream_cause(error)); }
					let code = debits.refused.get().map_or("upstream_interrupted", |refusal| refusal.code);
					completion.finish(Some(code)).await; Err(io_error(code))?; unreachable!()
				}
			};
			debits.count(Direction::Received, bytes.len());
			yield Frame::data(bytes);
			let debit = tokio::select! {
				refusal = stopped(&mut active_rx) => Err((refusal, true)),
				result = debits.flush_due(Direction::Received, deadline) => result.map_err(|refusal| (refusal, false)),
			};
			if let Err((refusal, interrupted)) = debit {
				if interrupted { completion.interrupted(refusal.code); } else { completion.finish(Some(refusal.code)).await; }
				Err(io_error(refusal.code))?;
			}
		}
	};
	// Drive the upstream independently of downstream backpressure. Otherwise a
	// client that stops reading also prevents the stop check from dropping the
	// upstream response. The single-frame channel bounds buffered output.
	let (sender, receiver) = tokio::sync::mpsc::channel(1);
	tokio::spawn(async move {
		let interrupted = {
			let mut output = Box::pin(output);
			let stop = loop {
				let frame = tokio::select! {
					refusal = stopped(&mut bridge_active) => break Some(refusal),
					frame = output.next() => frame,
				};
				let Some(frame) = frame else { break None };
				tokio::select! {
					refusal = stopped(&mut bridge_active) => break Some(refusal),
					result = sender.send(frame) => if result.is_err() { break None },
				}
			};
			// Dropping `output` below writes the request's record. The stream
			// never saw this stop (the bridge won the race), so name it here or
			// the record would blame a client that went away.
			if let Some(refusal) = stop {
				let _ = bridge_debits.interrupted.set(refusal.code);
			}
			stop
		};
		if let Some(refusal) = interrupted {
			let _ = sender.send(Err(io_error(refusal.code))).await;
		}
	});
	let output = futures_util::stream::unfold(receiver, |mut receiver| async move {
		receiver.recv().await.map(|frame| (frame, receiver))
	});
	let mut result = Response::new(StreamBody::new(output).boxed_unsync());
	*result.status_mut() = status;
	*result.headers_mut() = headers;
	result
}

#[cfg(test)]
mod tests;

mod progress;
mod proxy;
