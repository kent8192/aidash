//! Management use cases.
use crate::{
	Error, Result,
	apps::execution::serializers::runs::RunDetails,
	apps::execution::serializers::state::StateResponse,
	apps::federation::remote::serializers::actions::SentResponse,
	apps::identity::serializers::session::AccessProfile,
	apps::registry::serializers::installations::Installation,
	apps::workspaces::serializers::tasks::PageQuery,
	authorization::{execution, identity::Actor, interaction},
	domain::*,
	federation::Federation,
	registry::Search,
};
use futures_util::StreamExt;
use http::HeaderMap;
use reinhardt::http::StreamBody;
use reinhardt::injectable;
use reinhardt::query::Expr;
use reinhardt::query::PostgresQueryBuilder;
use reinhardt::query::Query;
use reinhardt::query::QueryStatementBuilder as _;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::apps::execution::serializers::management::ClaimInput;
use crate::apps::execution::serializers::management::ControlInput;
use crate::apps::execution::serializers::management::EventQuery;
use crate::apps::workspaces::serializers::management::MessageInput;

use crate::apps::identity::services::http_auth::scoped;

#[derive(Clone)]
pub struct HarnessManagement {
	runtime: Federation,
	event_streams: crate::sse::Service,
	catalog: std::sync::Arc<dyn aidash_application::ports::ModelCatalog>,
}
#[injectable(scope = "request")]
pub async fn provide(
	#[inject] runtime: Federation,
	#[inject] event_streams: reinhardt::Depends<crate::sse::Service>,
) -> HarnessManagement {
	HarnessManagement {
		catalog: crate::bootstrap::model_catalog(runtime.client.clone()),
		runtime,
		event_streams: (*event_streams).clone(),
	}
}
impl HarnessManagement {
	pub(crate) async fn state(&self, actor: Actor) -> Result<Response> {
		let f = self.runtime.clone();

		if let Some(scope) = scoped(&f, actor) {
			let state = scope.state(f.config.identity(vec![])).await?;
			return crate::marketplace::state_response(&scope.store, &scope.identity, state).await;
		}
		let records = f.registry.list(&Search::default()).await?;
		// Scan newest first until 100 visible events are collected. Hidden
		// required-Home events may occupy any number of candidate pages.
		let mut events = Vec::new();
		let mut before = i64::MAX;
		loop {
			let batch: Vec<crate::domain::Event> = crate::database::query_as(
				&reinhardt::query::Query::select()
					.column(reinhardt::query::ColumnRef::Asterisk)
					.from(reinhardt::query::Alias::new("events"))
					.and_where(reinhardt::query::Expr::cust("sequence < $1"))
					.order_by(
						reinhardt::query::Alias::new("sequence"),
						reinhardt::query::Order::Desc,
					)
					.limit(500)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.bind(before)
			.fetch_all(&f.store.pool)
			.await?;
			if batch.is_empty() {
				break;
			}
			before = batch.last().expect("nonempty event batch").sequence;
			let exhausted = batch.len() < 500;
			events.extend(
				crate::authorization::remote::operator::filter_events(
					&mut crate::database::native::begin(&f.store.pool).await?,
					batch,
				)
				.await?,
			);
			if events.len() >= 100 || exhausted {
				break;
			}
		}
		events.truncate(100);
		events.reverse();
		let human: Vec<HumanRequest> = crate::database::query_as(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
				))
				.from_as(
					reinhardt::query::Alias::new("human_requests"),
					reinhardt::query::Alias::new("h"),
				)
				.and_where(crate::authorization::remote::operator::state_visible(
					"h.workspace_id",
				))
				.order_by_expr(
					reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(
						reinhardt::query::Alias::new("created_at"),
					)),
					reinhardt::query::Order::Desc,
				)
				.limit(500)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_all(&f.store.pool)
		.await?;
		let conversations: Vec<Conversation> = crate::database::query_as(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
				))
				.from_as(
					reinhardt::query::Alias::new("conversations"),
					reinhardt::query::Alias::new("c"),
				)
				.and_where(crate::authorization::remote::operator::state_visible(
					"c.workspace_id",
				))
				.order_by_expr(
					reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(
						reinhardt::query::Alias::new("created_at"),
					)),
					reinhardt::query::Order::Desc,
				)
				.limit(500)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_all(&f.store.pool)
		.await?;
		let artifacts: Vec<Artifact> = crate::database::query_as(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
				))
				.from_as(
					reinhardt::query::Alias::new("artifacts"),
					reinhardt::query::Alias::new("a"),
				)
				.and_where(crate::authorization::remote::operator::state_visible(
					"a.workspace_id",
				))
				.order_by_expr(
					reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(
						reinhardt::query::Alias::new("created_at"),
					)),
					reinhardt::query::Order::Desc,
				)
				.limit(500)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_all(&f.store.pool)
		.await?;
		let installations: Vec<Installation> = crate::database::native::query_as(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
				))
				.from(reinhardt::query::Alias::new("installations"))
				.order_by_expr(
					reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(
						reinhardt::query::Alias::new("installed_at"),
					)),
					reinhardt::query::Order::Desc,
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_all(&f.store.pool)
		.await?;
		let mut state = StateResponse {
			access: AccessProfile::Operator,
			node: f.config.identity(vec![]),
			registry: records,
			workspaces: f.store.workspaces().await?,
			tasks: crate::database::query_as(
				&reinhardt::query::Query::select()
					.column(reinhardt::query::ColumnRef::Asterisk)
					.from_as(
						reinhardt::query::Alias::new("tasks"),
						reinhardt::query::Alias::new("t"),
					)
					.and_where(crate::authorization::remote::operator::state_visible(
						"t.workspace_id",
					))
					.order_by(
						reinhardt::query::Alias::new("created_at"),
						reinhardt::query::Order::Desc,
					)
					.order_by(
						reinhardt::query::Alias::new("id"),
						reinhardt::query::Order::Desc,
					)
					.limit(500)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_all(&f.store.pool)
			.await?,
			runs: crate::database::query_as::<crate::domain::run_state::RawRun>(
				&reinhardt::query::Query::select()
					.column(reinhardt::query::ColumnRef::Asterisk)
					.from_as(
						reinhardt::query::Alias::new("runs"),
						reinhardt::query::Alias::new("r"),
					)
					.and_where(crate::authorization::remote::operator::state_visible(
						"r.workspace_id",
					))
					.order_by(
						reinhardt::query::Alias::new("updated_at"),
						reinhardt::query::Order::Desc,
					)
					.limit(500)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_all(&f.store.pool)
			.await?
			.into_iter()
			.map(crate::domain::run_state::RawRun::inspect)
			.collect(),
			human_requests: human,
			conversations,
			peers: f.peers().await?,
			events,
			artifacts,
			installations,
		};
		crate::authorization::remote::operator::filter_state(
			&mut crate::database::native::begin(&f.store.pool).await?,
			&mut state,
		)
		.await?;
		Ok(Response::ok().with_json(&state)?)
	}
	pub(crate) async fn openrouter_models(&self) -> Result<Vec<crate::openrouter::CatalogModel>> {
		Ok(self.catalog.models().await?)
	}
	pub(crate) async fn task_claim(
		&self,
		actor: Actor,
		id: Uuid,
		input: ClaimInput,
	) -> Result<Task> {
		let f = self.runtime.clone();
		if let Actor::Subject(identity) = actor {
			return execution::claim(&f, &identity, id, input.revision, &input.agent).await;
		}
		let entry = f
			.registry
			.get(&input.agent.id, &input.agent.version)
			.await?;
		let owner = qualified_agent(&f.config.node_id, &entry.id, &entry.version);
		let task = f.store.claim(id, input.revision, &owner, &entry).await?;
		f.store
			.accept_run(&task, &f.config.node_id, &entry.id, &entry.version)
			.await?;
		Ok(task)
	}
	pub(crate) async fn run_get(
		&self,
		actor: Actor,
		id: Uuid,
		page: PageQuery,
	) -> Result<RunDetails> {
		let f = self.runtime.clone();
		if let Actor::Subject(identity) = actor {
			let mut details = execution::details_page(&f, &identity, id, page.offset).await?;
			details.media_input_routes = f.run_media_input_routes(&details.run).await?;
			return Ok(details);
		}
		let run = f.store.inspect_run(id).await?;
		crate::authorization::remote::operator::require(
			&mut crate::database::native::begin(&f.store.pool).await?,
			run.workspace_id,
		)
		.await?;
		let mut details = f.store.run_details(id, page.offset).await?;
		details.media_input_routes = f.run_media_input_routes(&details.run).await?;
		Ok(details)
	}
	pub(crate) async fn run_control(
		&self,
		actor: Actor,
		browser: Option<crate::dashboard_auth::BrowserOrigin>,
		id: Uuid,
		input: ControlInput,
	) -> Result<crate::domain::RunInspection> {
		let f = self.runtime.clone();
		if matches!(actor, Actor::Operator)
			&& browser.is_some()
			&& input.action == crate::domain::RunControlAction::Resume
		{
			return Err(Error::Forbidden);
		}
		if let Actor::Subject(identity) = actor {
			return execution::control(&f, &identity, id, input.action).await;
		}
		let existing = f.store.inspect_run(id).await?;
		crate::authorization::remote::operator::require(
			&mut crate::database::native::begin(&f.store.pool).await?,
			existing.workspace_id,
		)
		.await?;
		let r = f.store.control(id, input.action).await?;
		f.notify.notify_waiters();
		Ok(r)
	}
	pub(crate) async fn run_message(
		&self,
		actor: Actor,
		id: Uuid,
		input: MessageInput,
	) -> Result<SentResponse> {
		let f = self.runtime.clone();
		if !input.attachment_ids.is_empty() {
			return self.run_media_message(actor, id, input).await;
		}

		if let Actor::Subject(identity) = actor {
			interaction::message_keyed(&f, &identity, id, &input.content, input.idempotency_key)
				.await?;
			return Ok(SentResponse { sent: true });
		}
		let run = f.store.run(id).await?;
		let key = format!(
			"human:{id}:{}",
			input.idempotency_key.unwrap_or_else(Uuid::new_v4)
		);
		f.require_terminal_safe_delivery(&run).await?;
		let limit = f.run_message_limit(&run).await?;
		f.admit_run_message(&run, "human", &input.content, &key, limit)
			.await?;
		if let Err(error) = f.deliver_run_messages(&run).await {
			tracing::warn!(run_id=%id, %error, "accepted run message awaits home delivery");
		}
		f.notify.notify_waiters();
		Ok(SentResponse { sent: true })
	}
	pub(crate) async fn human_answer(
		&self,
		actor: Actor,
		id: Uuid,
		response: Value,
	) -> Result<HumanRequest> {
		let f = self.runtime.clone();
		if let Actor::Subject(identity) = actor {
			return interaction::answer(&f, &identity, id, response).await;
		}
		let request = f.store.human_request_by_id(id).await?;
		let run = f.store.run(request.run_id).await?;
		if run.home_node != f.config.node_id
			&& crate::authorization::peer::admission::run_grant(&f.store, &run.metadata())
				.await?
				.is_none()
		{
			crate::federation::Home::new(f.clone(), run)
				.answer_home_human(id, response.clone())
				.await?;
		}
		let h = f.store.answer(id, response).await?;
		f.notify.notify_waiters();
		Ok(h)
	}
	pub(crate) async fn events(&self, actor: Actor, q: EventQuery) -> Result<Response> {
		let f = self.runtime.clone();

		if let Some(scope) = scoped(&f, actor) {
			let (events, cursor) = scope
				.events_with_cursor(q.after, q.workspace_id, 500)
				.await?;
			let mut response =
				crate::marketplace::events::response(&scope.store, &scope.identity, events).await?;
			response
				.headers
				.insert("x-aidash-event-cursor", cursor.into());
			return Ok(response);
		}
		let mut cursor = q.after.max(0);
		let mut visible = Vec::new();
		let mut connection = crate::database::native::begin(&f.store.pool).await?;
		if let Some(workspace) = q.workspace_id
			&& !crate::authorization::remote::operator::visible(&mut connection, workspace).await?
		{
			let mut response = Response::ok().with_json(&visible)?;
			response
				.headers
				.insert("x-aidash-event-cursor", cursor.into());
			return Ok(response);
		}
		loop {
			let page = f.store.events(cursor, q.workspace_id, 500).await?;
			let exhausted = page.len() < 500;
			if let Some(last) = page.last() {
				cursor = last.sequence;
			}
			visible.extend(
				crate::authorization::remote::operator::filter_events(&mut connection, page)
					.await?,
			);
			if visible.len() >= 500 || exhausted {
				if visible.len() >= 500 {
					visible.truncate(500);
					cursor = visible.last().expect("full event page").sequence;
				}
				break;
			}
		}
		let mut response = Response::ok().with_json(&visible)?;
		response
			.headers
			.insert("x-aidash-event-cursor", cursor.into());
		Ok(response)
	}

	pub(crate) async fn stream(
		&self,
		actor: Actor,
		browser: Option<crate::dashboard_auth::BrowserOrigin>,
		headers: HeaderMap,
		q: EventQuery,
		lease: Option<crate::http::SseLeaseHandle>,
	) -> Result<StreamBody> {
		let cursor = headers
			.get("last-event-id")
			.and_then(|value| value.to_str().ok())
			.and_then(|value| value.parse().ok())
			.unwrap_or(q.after);
		let mut stream = self
			.event_streams
			.open(
				self.runtime.clone(),
				crate::sse::StreamRequest {
					actor,
					browser,
					headers,
					cursor,
					workspace: q.workspace_id,
					lease,
				},
			)
			.await?;
		Ok(Box::pin(async_stream::stream! {
			let interval = std::time::Duration::from_secs(15);
			let mut keepalive = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
			keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
			loop {
				tokio::select! {
					biased;
					frame = stream.next() => match frame {
						Some(Ok(frame)) => yield Ok(frame.into_bytes()),
						Some(Err(error)) => match error {},
						None => return,
					},
					_ = keepalive.tick() => yield Ok(bytes::Bytes::from_static(b":\n\n")),
				}
			}
		}))
	}
	pub(crate) async fn health(&self) -> Result<Value> {
		let f = self.runtime.clone();
		crate::database::native::query(
			&Query::select()
				.expr(Expr::cust("1"))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&f.store.pool)
		.await?;
		Ok(json!({"status":"ok","node_id":f.config.node_id}))
	}
	pub(crate) async fn run_media_message(
		&self,
		actor: Actor,
		id: Uuid,
		input: MessageInput,
	) -> Result<SentResponse> {
		let f = self.runtime.clone();
		let run = match &actor {
			Actor::Subject(identity) => {
				let mut access =
					crate::authorization::access::Access::begin(&f.store, identity).await?;
				let result = access.run_for_interaction(id).await;
				access.finish(result).await?
			}
			Actor::Operator => f.store.run(id).await?,
		};
		let external = input.idempotency_key.unwrap_or_else(Uuid::new_v4);
		let key = match &actor {
			Actor::Subject(identity) => interaction::run_message_key(identity, id, external),
			Actor::Operator => format!("human:{id}:{external}"),
		};
		let mut lease = crate::collaboration::access::Lease::begin_message_create(
			&f.store,
			actor,
			run.workspace_id,
		)
		.await?;
		let preflight = async {
			if let Some(access) = lease.access_mut() {
				let visible = access.run_for_interaction(id).await?;
				if visible.workspace_id != run.workspace_id {
					return Err(Error::Forbidden);
				}
				access
					.require(&access.resource("run", id, json!({})), "run.message")
					.await?;
			}
			if run.home_node != f.config.node_id {
				return Err(Error::Invalid(
					"media run messages require a local run".into(),
				));
			}
			Ok(())
		}
		.await;
		if let Err(error) = preflight {
			return lease.finish(Err(error)).await;
		}
		let sender = lease.sender();
		let principal = lease.principal();
		let limit = f.run_message_limit(&run).await?;
		let mut lease = lease.into_native()?;
		let result = async {
			let (existing, record) = crate::apps::execution::models::RunInput::admit_media(
				lease.tx(),
				&f.config.node_id,
				id,
				&sender,
				&input.content,
				&key,
				limit,
			)
			.await?;
			let message = record
				.message_id
				.ok_or_else(|| Error::Conflict("media input has no local message".into()))?;
			let attachments =
				crate::apps::workspaces::models::ChannelAttachment::attach_run_media_in(
					lease.tx(),
					run.workspace_id,
					message,
					&principal,
					&input.attachment_ids,
				)
				.await?;
			if existing {
				return Ok(());
			}
			let mut parts = Vec::with_capacity(attachments.len() * 2);
			for attachment in &attachments {
				parts.push(crate::provider::ContentPart::Text(format!(
					"Run message {} attachment: {}",
					record.seq, attachment.filename
				)));
				parts.push(crate::provider::ContentPart::from_media(
					&attachment.media_type,
					attachment.content.clone(),
				)?);
			}
			let agent = f
				.registry
				.get_for_run(&run, &run.agent_id, &run.agent_version)
				.await?;
			let agent: crate::registry::AgentConfig = serde_json::from_value(agent.config)?;
			let model = f
				.registry
				.get_for_run(&run, &agent.model.id, &agent.model.version)
				.await?;
			let model: crate::registry::ModelConfig = serde_json::from_value(model.config)?;
			model.require_media_types(
				attachments
					.iter()
					.map(|attachment| attachment.media_type.as_str()),
			)?;
			let headroom = f.run_request_headroom(&run).await?;
			let request = crate::provider::ModelRequest {
				instructions: String::new(),
				context: json!({"run_message":input.content}).into(),
				tools: Vec::new(),
				max_output_tokens: 0,
				response_format: None,
				content_parts: parts,
				cache_scope: None,
			};
			request.validate()?;
			crate::generation::budget::Reservation::check_request(headroom, &request)?;
			Ok(())
		}
		.await;
		lease.finish(result).await?;
		f.notify.notify_waiters();
		Ok(SentResponse { sent: true })
	}
}

use reinhardt::Response;
