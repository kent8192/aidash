//! Run-scoped Inference Progress stream.
//!
//! Delivery reuses the event stream's connection control: a visibility lease
//! per frame, the run-scoped event authority before every frame, backpressure,
//! the authority monitor and the HTTP connection slot. Progress rows are read
//! from their own table without the event journal's advisory lock.
use super::*;
use crate::apps::execution::repositories::inference::{ProgressRow, RunTarget, TEXT, TOOL_CALL};
use aidash_domain::provider::progress::FLUSH_INTERVAL;
use serde_json::json;

/// Rows read per page; each row is at most 16 KiB of item data.
const PAGE: u64 = 100;
const CLOUD_EVENT_TYPE: &str = "aidash.inference.progress.v1";

pub(crate) struct RunStreamRequest {
	pub actor: Actor,
	pub browser: Option<BrowserOrigin>,
	pub headers: HeaderMap,
	pub run: Uuid,
	/// Last delivered `progress_seq`; `None` starts at the latest attempt.
	pub cursor: Option<i64>,
	pub lease: Option<crate::http::SseLeaseHandle>,
}

/// A stored progress row, or a range already removed by retention before it.
pub(crate) enum Delivery {
	Row(ProgressRow),
	Gap {
		attempt_id: Uuid,
		outcome: Option<String>,
		from: i64,
		to: i64,
	},
}

#[derive(Clone)]
struct RunContext {
	base: Context,
	run: Arc<RunTarget>,
}

impl RunContext {
	/// The run-scoped event every frame is authorized as, using the same
	/// workspace rule as the Run's own journal events.
	fn probe(&self) -> Event {
		let node = &self.base.f.store.node_id;
		Event {
			sequence: 0,
			id: Uuid::nil(),
			node_id: node.clone(),
			workspace_id: (&self.run.home_node == node).then_some(self.run.workspace_id),
			kind: "inference.progress".into(),
			data: json!({
				"run_id": self.run.id,
				"task_id": self.run.task_id,
				"workspace_id": self.run.workspace_id,
			}),
			created_at: chrono::Utc::now(),
		}
	}

	async fn visible(&self) -> Result<bool> {
		let probe = self.probe();
		match &self.base.scope {
			Some(scope) => scope.can_emit(&probe).await,
			None => {
				let mut connection =
					crate::database::native::begin(&self.base.f.store.pool).await?;
				crate::authorization::remote::operator::event_visible(&mut connection, &probe).await
			}
		}
	}

	fn render(&self, delivery: &Delivery) -> Frame {
		match delivery {
			Delivery::Row(row) => {
				let event = match row.kind.as_str() {
					TEXT => "inference.delta",
					TOOL_CALL => "inference.tool_call",
					// `started` rows report a pending outcome.
					_ => "inference.outcome",
				};
				let envelope = json!({
					"specversion": "1.0",
					"id": format!("{}:{}", self.run.id, row.seq),
					"source": self.base.f.store.node_id,
					"type": CLOUD_EVENT_TYPE,
					"subject": self.run.id,
					"time": row.created_at,
					"datacontenttype": "application/json",
					"data": {
						"attempt_id": row.attempt_id,
						"seq": row.seq,
						"kind": row.kind,
						"item": row.item,
					},
				});
				Frame::default()
					.id(row.seq.to_string())
					.event(event)
					.data(envelope.to_string())
			}
			Delivery::Gap {
				attempt_id,
				outcome,
				from,
				to,
			} => Frame::default().event("gap").data(
				json!({"attempt_id": attempt_id, "outcome": outcome, "from": from, "to": to})
					.to_string(),
			),
		}
	}
}

#[async_trait::async_trait]
impl Watch<Delivery> for RunContext {
	fn service(&self) -> &Service {
		&self.base.service
	}
	async fn idle_authority(&self) -> Result<()> {
		self.base.authority(false).await
	}
	async fn periodic_authority(&self) -> std::result::Result<(), Closed> {
		if !self.base.browser_authorized().await {
			return Err(Closed::Browser);
		}
		// Run-read and source revocation close an idle stream, not only the next frame.
		// A pending atomic transaction skips this tick; the next frame or tick
		// rechecks under a fresh lease.
		let _visibility = match ReadLease::begin(&self.base.f.store).await {
			Ok(visibility) => visibility,
			Err(Error::TransactionPending) => return Ok(()),
			Err(_) => return Err(Closed::Database),
		};
		match self.visible().await {
			Ok(true) => Ok(()),
			_ => Err(Closed::Authority),
		}
	}
	async fn frame(
		&self,
		delivery: &Delivery,
		control: &Control<Delivery>,
	) -> std::result::Result<Option<Frame>, Closed> {
		loop {
			self.base.service.gate_check();
			match ReadLease::begin(&self.base.f.store).await {
				Ok(visibility) => {
					control.waiting_for_gate(&self.base.service, false);
					if !self.base.browser_authorized().await {
						return Err(Closed::Browser);
					}
					if !self.visible().await.map_err(|_| Closed::Authority)? {
						return Err(Closed::Authority);
					}
					drop(visibility);
					return Ok(Some(self.render(delivery)));
				}
				Err(Error::TransactionPending) => {
					control.waiting_for_gate(&self.base.service, true);
					tokio::time::sleep(AUTH_INTERVAL).await;
				}
				Err(_) => return Err(Closed::Database),
			}
		}
	}
}

/// Sequences are dense per Run, so a jump means retention removed the rows in
/// between; they belong to the attempt whose outcome row follows.
fn deliveries(mut cursor: i64, rows: Vec<ProgressRow>) -> (Vec<Delivery>, i64) {
	let mut items = Vec::with_capacity(rows.len());
	for row in rows {
		if row.seq > cursor + 1 {
			items.push(Delivery::Gap {
				attempt_id: row.attempt_id,
				outcome: row.outcome.clone(),
				from: cursor + 1,
				to: row.seq - 1,
			});
		}
		cursor = row.seq;
		items.push(Delivery::Row(row));
	}
	(items, cursor)
}

impl Service {
	pub(crate) async fn open_run(
		&self,
		f: Federation,
		request: RunStreamRequest,
	) -> Result<EventStream<Delivery>> {
		let RunStreamRequest {
			actor,
			browser,
			headers,
			run,
			cursor,
			lease,
		} = request;
		// Subjects cannot distinguish a missing Run from one they may not read,
		// matching the subject run-detail read.
		let target = match f.store.inference_run(run).await {
			Err(Error::NotFound(_)) if matches!(actor, Actor::Subject(_)) => {
				return Err(Error::Forbidden);
			}
			target => target?,
		};
		// Register before validation and the initial read; marker events wake it.
		let observation = self.register(Some(target.workspace_id));
		let scope = match actor {
			Actor::Operator => None,
			Actor::Subject(identity) => Some(Workspaces {
				store: f.store.clone(),
				identity,
			}),
		};
		let context = RunContext {
			base: Context {
				f,
				scope,
				browser,
				headers,
				workspace: None,
				service: self.clone(),
			},
			run: Arc::new(target),
		};
		context.base.authority(true).await?;
		let cursor = {
			let _visibility = ReadLease::begin(&context.base.f.store).await?;
			if !context.visible().await? {
				return Err(Error::Forbidden);
			}
			match cursor {
				Some(cursor) if cursor >= 0 => cursor,
				_ => context
					.base
					.f
					.store
					.inference_bounds(run)
					.await?
					.0
					.map_or(0, |first| first - 1),
			}
		};
		let control = Arc::new(Control::new(lease));
		let (sender, pages) = mpsc::channel::<()>(1);
		let reader = read_run(
			context.clone(),
			control.clone(),
			observation,
			cursor,
			sender,
		);
		Ok(stream(context, control, reader, pages))
	}
}

/// Poll at the flush interval while an attempt is pending; otherwise wait for
/// a marker wakeup or the reconciliation fallback.
async fn read_run(
	context: RunContext,
	control: Arc<Control<Delivery>>,
	mut observation: Observation,
	mut cursor: i64,
	sender: mpsc::Sender<()>,
) -> Result<()> {
	let service = context.base.service.clone();
	let store = context.base.f.store.clone();
	let interval = service.inner.settings.reconcile_interval;
	loop {
		observation.take_changes();
		service.gate_check();
		let visibility = match ReadLease::begin(&store).await {
			Ok(lease) => lease,
			Err(Error::TransactionPending) => {
				control.waiting_for_gate(&service, true);
				tokio::time::sleep(AUTH_INTERVAL).await;
				continue;
			}
			Err(error) => return Err(error),
		};
		control.waiting_for_gate(&service, false);
		let rows = store.inference_page(context.run.id, cursor, PAGE).await?;
		let (_, pending) = store.inference_bounds(context.run.id).await?;
		drop(visibility);
		let more = rows.len() as u64 == PAGE;
		let (items, scanned) = deliveries(cursor, rows);
		if !items.is_empty() && !control.deliver(items, &sender).await {
			return Ok(());
		}
		cursor = scanned;
		if more {
			tokio::task::yield_now().await;
			continue;
		}
		let wait = if pending { FLUSH_INTERVAL } else { interval };
		tokio::select! {
			biased;
			_ = tokio::time::sleep(wait) => {},
			change = observation.receiver.changed() => {
				if change.is_err() { return Ok(()); }
			}
		}
	}
}
