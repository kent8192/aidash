//! Management use cases.
use crate::{
	Error, Result,
	apps::execution::serializers::runs::RunDetails,
	apps::execution::serializers::state::StateResponse,
	apps::federation::remote::serializers::actions::SentResponse,
	apps::identity::serializers::session::AccessProfile,
	apps::workspaces::serializers::tasks::PageQuery,
	authorization::{execution, identity::Actor, interaction},
	domain::*,
	federation::Federation,
};
use aidash_application::tenancy::TenancyScope;
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
use crate::apps::execution::serializers::management::InferenceStreamQuery;
use crate::apps::workspaces::serializers::management::MessageInput;

use crate::apps::identity::services::http_auth::scoped;

/// The one process-wide OpenRouter catalog, read by the model picker and by
/// the Data Protection impact preview (#963).
pub(crate) static OPENROUTER_CATALOG: aidash_application::catalog::CatalogCache =
	aidash_application::catalog::CatalogCache::new();

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
	pub(crate) async fn thread_runs(
		&self,
		actor: Actor,
		id: Uuid,
		q: crate::apps::execution::serializers::management::ThreadRunListQuery,
	) -> Result<aidash_application::authorization::bounded_reads::RunPage> {
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		let cursor = q
			.cursor
			.as_deref()
			.map(Uuid::parse_str)
			.transpose()
			.map_err(|_| Error::Invalid("invalid cursor".into()))?;
		let mut access =
			crate::authorization::access::Access::begin(&self.runtime.store, &identity).await?;
		// A list: a silent Node leaves its Runs unconfirmed instead of failing it (#770).
		access.lists = true;
		let result = aidash_application::authorization::bounded_reads::thread_runs(
			&mut crate::apps::identity::repositories::visibility::bounded_reads::Reads {
				federation: &self.runtime,
				access: &mut access,
				task_thread: None,
				selected_run: None,
				node: None,
			},
			id,
			q.live,
			cursor,
		)
		.await
		.map_err(Into::into);
		access.finish(result).await
	}
	pub(crate) async fn task_runs(
		&self,
		actor: Actor,
		id: Uuid,
		q: crate::apps::execution::serializers::management::RunListQuery,
	) -> Result<aidash_application::authorization::bounded_reads::RunPage> {
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		let cursor = q
			.cursor
			.as_deref()
			.map(Uuid::parse_str)
			.transpose()
			.map_err(|_| Error::Invalid("invalid cursor".into()))?;
		let mut access =
			crate::authorization::access::Access::begin(&self.runtime.store, &identity).await?;
		access.lists = true;
		let result = aidash_application::authorization::bounded_reads::runs(
			&mut crate::apps::identity::repositories::visibility::bounded_reads::Reads {
				federation: &self.runtime,
				access: &mut access,
				task_thread: None,
				selected_run: None,
				node: None,
			},
			id,
			cursor,
		)
		.await
		.map_err(Into::into);
		access.finish(result).await
	}
	pub(crate) async fn human_requests(
		&self,
		actor: Actor,
		q: crate::apps::execution::serializers::management::HumanListQuery,
	) -> Result<aidash_application::authorization::bounded_reads::HumanPage> {
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		let cursor = q
			.cursor
			.as_deref()
			.map(Uuid::parse_str)
			.transpose()
			.map_err(|_| Error::Invalid("invalid cursor".into()))?;
		let mut access =
			crate::authorization::access::Access::begin(&self.runtime.store, &identity).await?;
		access.lists = true;
		let result = aidash_application::authorization::bounded_reads::humans(
			&mut crate::apps::identity::repositories::visibility::bounded_reads::Reads {
				federation: &self.runtime,
				access: &mut access,
				task_thread: None,
				selected_run: None,
				node: q.node.as_deref(),
			},
			q.run,
			q.state,
			cursor,
		)
		.await
		.map_err(Into::into);
		access.finish(result).await
	}
	pub(crate) async fn state(
		&self,
		actor: Actor,
		query: crate::apps::execution::serializers::state::StateQuery,
	) -> Result<Response> {
		let f = self.runtime.clone();

		if let Some(scope) = scoped(&f, actor) {
			let state = scope
				.state(
					crate::apps::federation::peer::services::identity::publication(&f).await?,
					&f,
					query.node.as_deref(),
				)
				.await?;
			return crate::marketplace::state_response(&scope.store, &scope.identity, state).await;
		}
		// A Deployment Administrator holds no Tenant authority (#484, #301): the
		// state carries the access profile and the Deployment identity only.
		let state = StateResponse {
			offering: crate::bootstrap::build::offering().as_str().to_owned(),
			access: AccessProfile::DeploymentAdministrator,
			deployment: crate::apps::federation::peer::services::identity::publication(&f).await?,
			registry: Vec::new(),
			threads: Vec::new(),
			tasks: Vec::new(),
			runs: Vec::new(),
			human_requests: Vec::new(),
			conversations: Vec::new(),
			events: Vec::new(),
			artifacts: Vec::new(),
			unconfirmed: Vec::new(),
			next_cursors: Default::default(),
		};
		Ok(Response::ok().with_json(&state)?)
	}
	pub(crate) async fn thread_page(
		&self,
		actor: Actor,
		query: crate::apps::execution::serializers::state::ThreadPageQuery,
	) -> Result<crate::apps::execution::serializers::state::ThreadPage> {
		use aidash_application::ports::authorization::state::ThreadStateScope;
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		let cursor = query
			.cursor
			.as_deref()
			.map(Uuid::parse_str)
			.transpose()
			.map_err(|_| Error::Invalid("invalid cursor".into()))?;
		let mut access =
			crate::authorization::access::Access::begin(&self.runtime.store, &identity).await?;
		let result: Result<_> = async {
			let mut scope = crate::apps::identity::repositories::visibility::state::StateReads {
				access: &mut access,
				federation: &self.runtime,
			};
			let ids = scope
				.thread_candidates(query.node.as_deref(), cursor)
				.await?;
			let next_cursor = (ids.len()
				== aidash_application::authorization::bounded_reads::PAGE_SIZE)
				.then(|| ids.last().unwrap().to_string());
			let mut visible = Vec::new();
			for id in ids {
				if scope.allowed(id, "thread.read").await? {
					visible.push(id);
				}
			}
			Ok(crate::apps::execution::serializers::state::ThreadPage {
				items: scope.thread_rows(&visible).await?,
				next_cursor,
			})
		}
		.await;
		access.finish(result).await
	}
	/// The OpenRouter catalog for model definitions, which any member may
	/// read, from one process-wide copy (#362).
	pub(crate) async fn openrouter_models(
		&self,
	) -> Result<aidash_domain::catalog::CatalogSnapshot> {
		Ok(OPENROUTER_CATALOG
			.get(self.catalog.as_ref(), chrono::Utc::now)
			.await?)
	}
	pub(crate) async fn task_claim(
		&self,
		actor: Actor,
		id: Uuid,
		input: ClaimInput,
	) -> Result<Task> {
		let f = self.runtime.clone();
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		execution::claim(&f, &identity, id, input.revision, &input.agent).await
	}
	pub(crate) async fn run_get(
		&self,
		actor: Actor,
		id: Uuid,
		page: PageQuery,
	) -> Result<RunDetails> {
		let f = self.runtime.clone();
		// Run details are Tenant content: an administrator reads none (#484).
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		let mut details = execution::details_page(&f, &identity, id, page.offset).await?;
		details.media_input_routes = f.run_media_input_routes(&details.run).await?;
		Ok(details)
	}
	pub(crate) async fn run_subnodes(
		&self,
		actor: Actor,
		id: Uuid,
	) -> Result<Vec<crate::apps::execution::serializers::subnodes::SubnodeView>> {
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		crate::apps::identity::services::execution::subnodes::list(&self.runtime, &identity, id)
			.await
	}
	pub(crate) async fn dissolve_subnode(
		&self,
		actor: Actor,
		id: Uuid,
	) -> Result<crate::apps::execution::serializers::subnodes::Dissolved> {
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		crate::apps::identity::services::execution::subnodes::dissolve(&self.runtime, &identity, id)
			.await
	}
	pub(crate) async fn run_control(
		&self,
		actor: Actor,
		id: Uuid,
		input: ControlInput,
	) -> Result<execution::management::RunManagement> {
		use execution::management::{self, ManagementAction, RunManagementInput};
		let f = self.runtime.clone();
		// A Deployment Administrator controls a Run only through a Safety Stop
		// (`/management`, ADR 0101): no resume, and no content returned.
		// The refusal comes before any mutation or serialization.
		let Actor::Subject(identity) = &actor else {
			return Err(Error::Forbidden);
		};
		// Stopping never depends on what the Run has read, and there is one
		// check for it; resuming does depend on it. Resuming waits behind an
		// Authority Change at the boundary, before the visibility lease begins;
		// pausing and cancelling do not (ADR 0121).
		let action = match input.action {
			crate::domain::RunControlAction::Resume => {
				return Ok(execution::resume(&f, identity, id).await?.into());
			}
			crate::domain::RunControlAction::Pause => ManagementAction::Pause,
			crate::domain::RunControlAction::Cancel => ManagementAction::Cancel,
		};
		management::control(f, actor, id, RunManagementInput { action }).await
	}
	/// The caller's Runs that an Account suspension paused. A Deployment
	/// Administrator has no Runs of its own and is refused, as on resume.
	pub(crate) async fn suspended_runs(
		&self,
		actor: Actor,
	) -> Result<crate::apps::execution::serializers::management::SuspendedRuns> {
		let Actor::Subject(identity) = &actor else {
			return Err(Error::Forbidden);
		};
		Ok(
			crate::apps::execution::serializers::management::SuspendedRuns {
				runs: execution::suspension_paused(&self.runtime, identity).await?,
			},
		)
	}
	/// Resumes each of the caller's Runs that an Account suspension paused,
	/// one at a time through the manual resume and all of its checks, and
	/// reports each Run (#876 decision 17). A Deployment Administrator has no
	/// Runs of its own and is refused, as on `/control`.
	pub(crate) async fn resume_suspended(
		&self,
		actor: Actor,
	) -> Result<crate::apps::execution::serializers::management::SuspendedRunsResume> {
		use crate::apps::execution::serializers::management::{
			SuspendedRunResume, SuspendedRunsResume,
		};
		let Actor::Subject(identity) = &actor else {
			return Err(Error::Forbidden);
		};
		let f = &self.runtime;
		let mut runs = Vec::new();
		for id in execution::suspension_paused(f, identity).await? {
			runs.push(match execution::resume(f, identity, id).await {
				Ok(_) => SuspendedRunResume::resumed(id),
				Err(error) => SuspendedRunResume::refused(id, error),
			});
		}
		Ok(SuspendedRunsResume { runs })
	}
	/// Declining answers as `/control` does. A Deployment Administrator never declines.
	pub(crate) async fn run_decline_call(
		&self,
		actor: Actor,
		id: Uuid,
		input: crate::apps::execution::serializers::management::DeclineCallInput,
	) -> Result<execution::management::RunManagement> {
		let Actor::Subject(identity) = &actor else {
			return Err(Error::Forbidden);
		};
		Ok(
			execution::decline_call(&self.runtime, identity, id, &input.call, input.rest_of_run)
				.await?
				.into(),
		)
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

		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		interaction::message_keyed(&f, &identity, id, &input.content, input.idempotency_key)
			.await?;
		Ok(SentResponse { sent: true })
	}
	pub(crate) async fn human_answer(
		&self,
		actor: Actor,
		id: Uuid,
		response: Value,
	) -> Result<HumanRequest> {
		let f = self.runtime.clone();
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		interaction::answer(&f, &identity, id, response).await
	}
	pub(crate) async fn events(&self, actor: Actor, q: EventQuery) -> Result<Response> {
		let f = self.runtime.clone();
		// Events are Tenant content: an administrator reads none (#484).
		let Some(scope) = scoped(&f, actor) else {
			return Err(Error::Forbidden);
		};
		let (events, cursor, unconfirmed) =
			scope.events_with_cursor(q.after, q.thread_id, 500).await?;
		let mut response = crate::marketplace::events::response(
			&scope.store,
			&scope.identity,
			events,
			unconfirmed,
		)
		.await?;
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
		let stream = self
			.event_streams
			.open(
				self.runtime.clone(),
				crate::sse::StreamRequest {
					actor,
					browser,
					headers,
					cursor,
					thread: q.thread_id,
					lease,
				},
			)
			.await?;
		Ok(keepalive(stream))
	}

	/// Display-only Inference Progress of one Run, resumable by `progress_seq`.
	pub(crate) async fn run_inference_stream(
		&self,
		actor: Actor,
		browser: Option<crate::dashboard_auth::BrowserOrigin>,
		headers: HeaderMap,
		run: Uuid,
		q: InferenceStreamQuery,
		lease: Option<crate::http::SseLeaseHandle>,
	) -> Result<StreamBody> {
		let cursor = headers
			.get("last-event-id")
			.and_then(|value| value.to_str().ok())
			.and_then(|value| value.parse().ok())
			.or(q.after);
		let stream = self
			.event_streams
			.open_run(
				self.runtime.clone(),
				crate::sse::RunStreamRequest {
					actor,
					browser,
					headers,
					run,
					cursor,
					lease,
				},
			)
			.await?;
		Ok(keepalive(stream))
	}
	pub(crate) async fn health(&self) -> Result<Value> {
		let f = self.runtime.clone();
		crate::database::native::query(
			&Query::select()
				.expr(Expr::cust("1"))
				.to_string(PostgresQueryBuilder),
		)
		.execute(f.store.pool.in_scope(&TenancyScope::NoTenant))
		.await?;
		Ok(json!({"status":"ok","deployment":f.config.deployment}))
	}
	pub(crate) async fn run_media_message(
		&self,
		actor: Actor,
		id: Uuid,
		input: MessageInput,
	) -> Result<SentResponse> {
		let f = self.runtime.clone();
		let Actor::Subject(identity) = &actor else {
			return Err(Error::Forbidden);
		};
		let run = {
			let mut access =
				crate::authorization::access::Access::begin(&f.store, identity).await?;
			let result = access.run_for_interaction(id).await;
			access.finish(result).await?
		};
		let external = input.idempotency_key.unwrap_or_else(Uuid::new_v4);
		let key = interaction::run_message_key(identity, id, external);
		let mut lease = crate::collaboration::access::Lease::begin_message_create(
			&f.store,
			actor,
			run.thread_id,
		)
		.await?;
		let preflight = async {
			let access = lease.access_mut();
			let visible = access.run_for_interaction(id).await?;
			if visible.thread_id != run.thread_id {
				return Err(Error::Forbidden);
			}
			access
				.require(&access.resource("run", id, json!({})), "run.message")
				.await?;
			if !run.is_local(f.config.deployment.as_str()) {
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
		let mut lease = lease.into_native().await?;
		use crate::apps::execution::repositories::model_calls;
		use aidash_domain::model_call::{Dispatch, Outcome, Purpose};
		let mut refused = None;
		let result = async {
			let (existing, record) = crate::apps::execution::models::RunInput::admit_media(
				lease.tx(),
				f.config.deployment.as_str(),
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
			if existing {
				crate::apps::workspaces::models::ChannelAttachment::attach_run_media_in(
					lease.tx(),
					run.thread_id,
					message,
					&principal,
					&input.attachment_ids,
				)
				.await?;
				return Ok(());
			}
			let tenant = f.store.run_tenant(run.id).await?;
			let agent = f
				.registry
				.get_for_run(&tenant, &run, &run.agent_id, &run.agent_version)
				.await?;
			let agent: crate::registry::AgentConfig = serde_json::from_value(agent.config)?;
			let model = f
				.registry
				.get_for_run(&tenant, &run, &agent.model.id, &agent.model.version)
				.await?;
			let model: crate::registry::ModelConfig =
				crate::registry::ModelConfig::from_stored(model.config)?;
			let headroom = f.run_request_headroom(&run).await?;
			let version = aidash_domain::projection::ProjectionVersion::Native;
			let batch = aidash_application::execution::media::load(
				&mut crate::apps::workspaces::repositories::media::ScopedMedia {
					access: lease.access_mut(),
					store: &f.store,
					pending: Some((message, input.attachment_ids.clone(), principal.clone())),
				},
				run.thread_id,
				&[(record.seq, message, headroom)],
				&model,
				version,
			)
			.await
			.map_err(|error| match error {
				aidash_application::Error::MediaRouteUnavailable {
					model: _,
					candidates,
				} => Error::Invalid(format!(
					"recipient model {} has no current media route for every selected format: {candidates:?}",
					model.model_id
				)),
				other => other.into(),
			})?;
			crate::apps::workspaces::models::ChannelAttachment::attach_run_media_in(
				lease.tx(),
				run.thread_id,
				message,
				&principal,
				&input.attachment_ids,
			)
			.await?;
			let parts = batch.parts;
			let request = aidash_application::execution::media::admission_request(
				&model,
				json!({"run_message":input.content}),
				parts,
			);
			let dispatch = Dispatch::new(Purpose::MediaCheck, &request)?;
			let owner = model_calls::Attribution::for_run(lease.tx(), id).await?;
			let attempt = Uuid::new_v4();
			let model = agent.model;
			let checked = (|| -> Result<()> {
				request.validate()?;
				crate::generation::budget::Reservation::check_request(headroom, &request)?;
				Ok(())
			})();
			if let Err(error) = checked {
				refused = Some((attempt, owner, model, dispatch));
				return Err(error);
			}
			model_calls::start(lease.tx(), attempt, &owner, &model, &dispatch).await?;
			model_calls::finish(lease.tx(), attempt, Outcome::Accepted, None).await
		}
		.await;
		let finished = lease.finish(result).await;
		// Preserve the refusal while rolling back the input and attachment rows.
		if let Some((attempt, owner, model, dispatch)) = refused {
			f.store
				.control_pool
				.atomic(
					&aidash_application::tenancy::TenancyScope::tenant(&owner.tenant)?,
					async |tx| {
						model_calls::start(tx, attempt, &owner, &model, &dispatch).await?;
						model_calls::finish(tx, attempt, Outcome::Refused, None).await
					},
				)
				.await?;
		}
		finished?;
		f.notify.notify_waiters();
		Ok(SentResponse { sent: true })
	}
}

use reinhardt::Response;

/// Frame a connection stream with the 15 s comment keepalive used by every SSE route.
fn keepalive<T: Send + 'static>(mut stream: crate::sse::EventStream<T>) -> StreamBody {
	Box::pin(async_stream::stream! {
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
	})
}
