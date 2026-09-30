use super::*;
use crate::{
	authorization::{identity::Actor, workspace::Workspaces},
	dashboard_auth::BrowserOrigin,
	domain::Event,
	federation::Federation,
	transactions::gate::ReadLease,
};
use axum::{
	http::{HeaderMap, Method},
	response::sse::Event as Frame,
};
use futures_util::{Stream, StreamExt, stream::BoxStream};
use std::{
	convert::Infallible,
	pin::Pin,
	task::{Context as TaskContext, Poll},
};
use tokio::{
	sync::{mpsc, oneshot},
	task::JoinHandle,
	time::{Instant, MissedTickBehavior},
};

const AUTH_INTERVAL: Duration = Duration::from_millis(250);
const BROWSER_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug)]
enum Closed {
	Authority,
	Browser,
	Database,
	Backpressure,
	Shutdown,
	Disconnected,
}
impl Closed {
	fn label(self) -> &'static str {
		match self {
			Self::Authority => "authority",
			Self::Browser => "browser",
			Self::Database => "database",
			Self::Backpressure => "backpressure",
			Self::Shutdown => "shutdown",
			Self::Disconnected => "disconnected",
		}
	}
}

struct Control {
	closed: watch::Sender<Option<Closed>>,
	pending: watch::Sender<Option<Instant>>,
	buffer: Mutex<Option<Page>>,
	gate_wait: Mutex<Option<GateWait>>,
	lease: Option<crate::http::SseLeaseHandle>,
}
impl Control {
	fn close(&self, reason: Closed) {
		let changed = self.closed.send_if_modified(|value| {
			if value.is_some() {
				false
			} else {
				*value = Some(reason);
				true
			}
		});
		if changed {
			self.gate_wait.lock().expect("SSE gate wait").take();
			// Release raw queued events even if the HTTP body is never polled again.
			if let Some(page) = self.buffer.lock().expect("SSE pending page").take() {
				metrics::gauge!("aidash_sse_pending_pages").decrement(1.0);
				metrics::gauge!("aidash_sse_pending_events").decrement(page.events.len() as f64);
			}
			metrics::counter!("aidash_sse_closed_total", "reason" => reason.label()).increment(1);
			if let Some(lease) = &self.lease {
				lease.release();
			}
		}
	}
	fn progress(&self, pending: bool) {
		self.pending.send_replace(pending.then(Instant::now));
	}
	fn waiting_for_gate(&self, service: &Service, waiting: bool) {
		let mut state = self.gate_wait.lock().expect("SSE gate wait");
		if waiting && self.closed.borrow().is_none() {
			state.get_or_insert_with(|| service.wait_for_gate());
		} else {
			state.take();
		}
	}
}

struct Tasks {
	control: Arc<Control>,
	reader: JoinHandle<()>,
	monitor: JoinHandle<()>,
}
impl Drop for Tasks {
	fn drop(&mut self) {
		self.control.close(Closed::Disconnected);
		self.reader.abort();
		self.monitor.abort();
	}
}

/// The task guard exists before the body is ever polled, so an unread response
/// still cancels its reader, authority monitor and registration on Drop.
pub struct EventStream {
	inner: BoxStream<'static, std::result::Result<Frame, Infallible>>,
	tasks: Tasks,
	ended: bool,
}
impl Stream for EventStream {
	type Item = std::result::Result<Frame, Infallible>;
	fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
		if self.ended {
			return Poll::Ready(None);
		}
		let result = if self.tasks.control.closed.borrow().is_none() {
			self.inner.as_mut().poll_next(cx)
		} else {
			Poll::Ready(None)
		};
		// A close can happen inside the body or while its await was pending.
		// Emit at most one terminal error, and never hand off a buffered frame
		// after observing the close.
		let closed = *self.tasks.control.closed.borrow();
		if let Some(reason) = closed {
			self.ended = true;
			return match reason {
				Closed::Database | Closed::Authority => Poll::Ready(Some(Ok(Frame::default()
					.event("error")
					.data("event stream interrupted")))),
				_ => Poll::Ready(None),
			};
		}
		result
	}
}

#[derive(Clone)]
struct Context {
	f: Federation,
	scope: Option<Workspaces>,
	browser: Option<BrowserOrigin>,
	headers: HeaderMap,
	workspace: Option<Uuid>,
	service: Service,
}
impl Context {
	async fn browser_authorized(&self) -> bool {
		let Some(origin) = &self.browser else {
			return true;
		};
		metrics::counter!("aidash_sse_browser_checks_total").increment(1);
		match crate::dashboard_auth::actor_from_headers(&self.f, &self.headers, &Method::GET).await
		{
			Ok((actor, current)) => {
				current.identity_id == origin.identity_id
					&& current.mapping_id == origin.mapping_id
					&& matches!(
						(self.scope.is_none(), actor),
						(true, Actor::Operator) | (false, Actor::Subject(_))
					)
			}
			Err(_) => false,
		}
	}
	async fn authority(&self, browser: bool) -> Result<()> {
		self.service
			.inner
			.counts
			.checks
			.fetch_add(1, Ordering::Relaxed);
		metrics::counter!("aidash_sse_authority_checks_total").increment(1);
		if let Some(scope) = &self.scope {
			scope.stream_authority(self.workspace).await?;
		}
		if browser && !self.browser_authorized().await {
			return Err(Error::Unauthorized);
		}
		Ok(())
	}
	async fn can_emit(
		&self,
		event: &Event,
		control: &Control,
	) -> std::result::Result<bool, Closed> {
		loop {
			self.service.gate_check();
			match ReadLease::begin(&self.f.store).await {
				Ok(visibility) => {
					control.waiting_for_gate(&self.service, false);
					if !self.browser_authorized().await {
						return Err(Closed::Browser);
					}
					let allowed = match &self.scope {
						Some(scope) => {
							scope.can_emit(event).await.map_err(|_| Closed::Authority)?
						}
						None => true,
					};
					drop(visibility);
					return Ok(allowed);
				}
				Err(Error::TransactionPending) => {
					control.waiting_for_gate(&self.service, true);
					tokio::time::sleep(AUTH_INTERVAL).await;
				}
				Err(_) => return Err(Closed::Database),
			}
		}
	}
}

struct Page {
	events: std::collections::VecDeque<Event>,
	consumed: oneshot::Sender<()>,
}

pub(crate) struct StreamRequest {
	pub actor: Actor,
	pub browser: Option<BrowserOrigin>,
	pub headers: HeaderMap,
	pub cursor: i64,
	pub workspace: Option<Uuid>,
	pub lease: Option<crate::http::SseLeaseHandle>,
}

impl Service {
	pub(crate) async fn open(&self, f: Federation, request: StreamRequest) -> Result<EventStream> {
		let StreamRequest {
			actor,
			browser,
			headers,
			mut cursor,
			workspace,
			lease,
		} = request;
		// Register before validation/high-water lookup/initial read.
		let observation = self.register(workspace);
		let scope = match actor {
			Actor::Operator => None,
			Actor::Subject(identity) => Some(Workspaces {
				store: f.store.clone(),
				identity,
			}),
		};
		let context = Context {
			f,
			scope,
			browser,
			headers,
			workspace,
			service: self.clone(),
		};
		context.authority(true).await?;
		if cursor < 0 {
			let _visibility = ReadLease::begin(&context.f.store).await?;
			self.record_read(Reasons::INITIAL);
			cursor = sqlx::query_scalar(
				&sea_orm::sea_query::Query::select()
					.expr(sea_orm::sea_query::Expr::cust("coalesce(max(sequence),0)"))
					.from(sea_orm::sea_query::Alias::new("events"))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.fetch_one(&context.f.store.pool)
			.await?;
		}
		let control = Arc::new(Control {
			closed: watch::channel(None).0,
			pending: watch::channel(None).0,
			buffer: Mutex::new(None),
			gate_wait: Mutex::new(None),
			lease,
		});
		let (sender, mut pages) = mpsc::channel::<()>(1);
		let reader_context = context.clone();
		let reader_control = control.clone();
		let reader = tokio::spawn(async move {
			let mut closed = reader_control.closed.subscribe();
			tokio::select! {
				biased;
				_ = closed.wait_for(|state| state.is_some()) => {},
				result = read(reader_context, reader_control.clone(), observation, cursor, sender) => {
					if result.is_err() { reader_control.close(Closed::Database); }
				}
			}
		});
		let monitor_context = context.clone();
		let monitor_control = control.clone();
		let monitor = tokio::spawn(async move {
			monitor(monitor_context, monitor_control).await;
		});
		let body_control = control.clone();
		let stream = async_stream::stream! {
			let mut closed = body_control.closed.subscribe();
			loop {
				tokio::select! {
					biased;
					_ = closed.wait_for(|state| state.is_some()) => return,
					page = pages.recv() => if page.is_none() { return; },
				};
				loop {
					let (event, consumed) = {
						let mut buffer = body_control.buffer.lock().expect("SSE pending page");
						let Some(page) = buffer.as_mut() else { break; };
						let event = page.events.pop_front().expect("nonempty pending page");
						metrics::gauge!("aidash_sse_pending_events").decrement(1.0);
						let consumed = page.events.is_empty().then(|| buffer.take().expect("pending page").consumed);
						if consumed.is_some() { metrics::gauge!("aidash_sse_pending_pages").decrement(1.0); }
						(event, consumed)
					};
					let last = consumed.is_some();
					// Database/visibility waits are server work, not a slow reader.
					body_control.progress(false);
					let allowed = tokio::select! {
						biased;
						_ = closed.wait_for(|state| state.is_some()) => return,
						allowed = context.can_emit(&event, &body_control) => allowed,
					};
					let allowed = match allowed {
						Ok(allowed) => allowed,
						Err(reason) => {
							body_control.close(reason);
							return;
						}
					};
					body_control.progress(!last);
					if allowed {
						let frame = Frame::default().id(event.sequence.to_string()).event("mesh").data(event.cloud_event().to_string());
						drop(event);
						// No await between the last frame's final check and handoff.
						if let Some(consumed) = consumed { let _ = consumed.send(()); }
						context.service.inner.counts.frames.fetch_add(1, Ordering::Relaxed);
						metrics::counter!("aidash_sse_frames_total").increment(1);
						yield Ok(frame);
					} else if let Some(consumed) = consumed {
						let _ = consumed.send(());
					}
					if last { break; }
				}
			}
		}.boxed();
		Ok(EventStream {
			inner: stream,
			tasks: Tasks {
				control,
				reader,
				monitor,
			},
			ended: false,
		})
	}
}

async fn read(
	context: Context,
	control: Arc<Control>,
	mut observation: Observation,
	mut cursor: i64,
	sender: mpsc::Sender<()>,
) -> Result<()> {
	let interval = context.service.inner.settings.reconcile_interval;
	let mut deadline = Instant::now() + interval;
	let mut reasons = Reasons::INITIAL;
	loop {
		reasons.add(observation.take_changes());
		if Instant::now() >= deadline {
			reasons.add(Reasons::FALLBACK);
		}
		context.service.gate_check();
		let visibility = match ReadLease::begin(&context.f.store).await {
			Ok(lease) => lease,
			Err(Error::TransactionPending) => {
				control.waiting_for_gate(&context.service, true);
				reasons.add(Reasons::GATE);
				tokio::time::sleep(AUTH_INTERVAL).await;
				continue;
			}
			Err(error) => return Err(error),
		};
		control.waiting_for_gate(&context.service, false);
		let (events, scanned, more) = if let Some(scope) = &context.scope {
			scope
				.stream_page(cursor, context.workspace, || {
					context.service.record_read(reasons)
				})
				.await?
		} else {
			context.service.record_read(reasons);
			let events = context
				.f
				.store
				.events(cursor, context.workspace, 100)
				.await?;
			let scanned = events.last().map_or(cursor, |event| event.sequence);
			let more = events.len() == 100;
			(events, scanned, more)
		};
		drop(visibility);
		context.service.reconciled();
		// Reads can satisfy a due reconciliation but hints cannot postpone it.
		if Instant::now() >= deadline {
			deadline = Instant::now() + interval;
		}
		if !events.is_empty() {
			let (consumed, receipt) = oneshot::channel();
			{
				let mut buffer = control.buffer.lock().expect("SSE pending page");
				if control.closed.borrow().is_some() {
					return Ok(());
				}
				metrics::gauge!("aidash_sse_pending_pages").increment(1.0);
				metrics::gauge!("aidash_sse_pending_events").increment(events.len() as f64);
				*buffer = Some(Page {
					events: events.into(),
					consumed,
				});
			}
			control.progress(true);
			if sender.send(()).await.is_err() || receipt.await.is_err() {
				return Ok(());
			}
		}
		cursor = scanned;
		if more {
			reasons = Reasons::BACKLOG;
			tokio::task::yield_now().await;
			continue;
		}
		reasons = tokio::select! {
			biased;
			_ = tokio::time::sleep_until(deadline) => Reasons::FALLBACK,
			change = observation.receiver.changed() => {
				if change.is_err() { return Ok(()); }
				Reasons(0)
			}
		};
	}
}

async fn monitor(context: Context, control: Arc<Control>) {
	let mut closed = control.closed.subscribe();
	let mut stopping = context.service.inner.stopping.subscribe();
	// Independent futures keep a stalled authority query from extending the
	// output deadline, and keep an idle/unpolled body from skipping revocation.
	tokio::select! {
		biased;
		_ = closed.wait_for(|state| state.is_some()) => {},
		_ = crate::lifecycle::stopped(&mut stopping) => control.close(Closed::Shutdown),
		reason = authority_monitor(&context) => control.close(reason),
		_ = output_monitor(&context, &control) => {
			context.service.inner.counts.backpressure.fetch_add(1, Ordering::Relaxed);
			control.close(Closed::Backpressure);
		},
	}
}

async fn authority_monitor(context: &Context) -> Closed {
	let mut ticks = tokio::time::interval(AUTH_INTERVAL);
	ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
	let mut browser_deadline = Instant::now() + BROWSER_INTERVAL;
	loop {
		ticks.tick().await;
		let browser = Instant::now() >= browser_deadline;
		let check = async {
			context
				.authority(false)
				.await
				.map_err(|_| Closed::Authority)?;
			if browser && !context.browser_authorized().await {
				return Err(Closed::Browser);
			}
			Ok(())
		};
		match tokio::time::timeout(BROWSER_INTERVAL, check).await {
			Ok(Ok(())) => {}
			Ok(Err(reason)) => return reason,
			Err(_) => return Closed::Authority,
		}
		if browser {
			browser_deadline = Instant::now() + BROWSER_INTERVAL;
		}
	}
}

async fn output_monitor(context: &Context, control: &Control) {
	let mut pending = control.pending.subscribe();
	loop {
		let deadline = (*pending.borrow_and_update())
			.map(|progress| progress + context.service.inner.settings.backpressure_timeout);
		tokio::select! {
			biased;
			_ = pending.changed() => {},
			_ = async {
				match deadline {
					Some(deadline) => tokio::time::sleep_until(deadline).await,
					None => std::future::pending().await,
				}
			} => return,
		}
	}
}
