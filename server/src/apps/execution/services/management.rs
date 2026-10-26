//! Management use cases.
use crate::{
	Error, Result,
	apps::execution::serializers::research::CitedEvidence,
	apps::execution::serializers::runs::RunDetails,
	apps::execution::serializers::state::StateResponse,
	apps::federation::remote::serializers::actions::SentResponse,
	apps::identity::serializers::session::AccessProfile,
	apps::workspaces::serializers::tasks::PageQuery,
	authorization::{execution, identity::Actor, interaction},
	domain::*,
	federation::Federation,
};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::provider::progress::MARKER_PREFIX;
use futures_util::StreamExt;
use http::HeaderMap;
use reinhardt::http::StreamBody;
use reinhardt::injectable;
use reinhardt::query::PostgresQueryBuilder;
use reinhardt::query::Query;
use reinhardt::query::QueryStatementBuilder as _;
use reinhardt::query::{Expr, ExprTrait as _};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::apps::execution::serializers::management::ClaimInput;
use crate::apps::execution::serializers::management::ControlInput;
use crate::apps::execution::serializers::management::EventQuery;
use crate::apps::execution::serializers::management::InferenceStreamQuery;
use crate::apps::workspaces::serializers::management::MessageInput;

use crate::apps::execution::repositories::research::Revocations;
use crate::apps::execution::serializers::research::{
	HistoryPage, RetentionInput, RevocationInput, RevocationRecord, TenantRetentionView,
};
use crate::apps::identity::repositories::retention::TenantRetention;
use crate::apps::identity::services::http_auth::scoped;

/// The dashboard identity of a Deployment Administrator acting in a browser.
fn administrator(
	actor: &Actor,
	browser: Option<crate::dashboard_auth::BrowserOrigin>,
) -> Option<Uuid> {
	matches!(actor, Actor::DeploymentAdministrator)
		.then(|| browser.map(|origin| origin.identity_id))
		.flatten()
}

#[derive(Clone)]
pub struct HarnessManagement {
	runtime: Federation,
	event_streams: crate::sse::Service,
	catalog: std::sync::Arc<dyn aidash_application::ports::ModelCatalog>,
}
/// A Deployment Administrator's reads across Tenants.
fn listing() -> TenancyScope {
	TenancyScope::Deployment(DeploymentScopeReason::DeploymentAdministratorList)
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
			let state = scope.state(f.config.deployment_identity()).await?;
			return crate::marketplace::state_response(&scope.store, &scope.identity, state).await;
		}
		let records: Vec<crate::registry::Entry> = Vec::new();
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
					.and_where(
						Expr::col(reinhardt::query::Alias::new("kind"))
							.not_like(Expr::value(format!("{MARKER_PREFIX}%"))),
					)
					.order_by(
						reinhardt::query::Alias::new("sequence"),
						reinhardt::query::Order::Desc,
					)
					.limit(500)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.bind(before)
			.fetch_all(f.store.pool.in_scope(&listing()))
			.await?;
			if batch.is_empty() {
				break;
			}
			before = batch.last().expect("nonempty event batch").sequence;
			let exhausted = batch.len() < 500;
			events.extend(
				crate::authorization::remote::deployment_administrator::filter_events(
					&mut f.store.pool.begin(&listing()).await?,
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
				.and_where(
					crate::authorization::remote::deployment_administrator::state_visible(
						"h.workspace_id",
					),
				)
				.order_by_expr(
					reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(
						reinhardt::query::Alias::new("created_at"),
					)),
					reinhardt::query::Order::Desc,
				)
				.limit(500)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_all(f.store.pool.in_scope(&listing()))
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
				.and_where(
					crate::authorization::remote::deployment_administrator::state_visible(
						"c.workspace_id",
					),
				)
				.order_by_expr(
					reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(
						reinhardt::query::Alias::new("created_at"),
					)),
					reinhardt::query::Order::Desc,
				)
				.limit(500)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_all(f.store.pool.in_scope(&listing()))
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
				.and_where(
					crate::authorization::remote::deployment_administrator::state_visible(
						"a.workspace_id",
					),
				)
				.order_by_expr(
					reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(
						reinhardt::query::Alias::new("created_at"),
					)),
					reinhardt::query::Order::Desc,
				)
				.limit(500)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_all(f.store.pool.in_scope(&listing()))
		.await?;
		let mut state = StateResponse {
			access: AccessProfile::DeploymentAdministrator,
			deployment: f.config.deployment_identity(),
			registry: records,
			workspaces: f.store.workspaces(&listing()).await?,
			tasks: crate::database::query_as(
				&reinhardt::query::Query::select()
					.column(reinhardt::query::ColumnRef::Asterisk)
					.from_as(
						reinhardt::query::Alias::new("tasks"),
						reinhardt::query::Alias::new("t"),
					)
					.and_where(
						crate::authorization::remote::deployment_administrator::state_visible(
							"t.workspace_id",
						),
					)
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
			.fetch_all(f.store.pool.in_scope(&listing()))
			.await?,
			runs: crate::database::query_as::<crate::domain::run_state::RawRun>(
				&reinhardt::query::Query::select()
					.column(reinhardt::query::ColumnRef::Asterisk)
					.from_as(
						reinhardt::query::Alias::new("runs"),
						reinhardt::query::Alias::new("r"),
					)
					.and_where(
						crate::authorization::remote::deployment_administrator::state_visible(
							"r.workspace_id",
						),
					)
					.order_by(
						reinhardt::query::Alias::new("updated_at"),
						reinhardt::query::Order::Desc,
					)
					.limit(500)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_all(f.store.pool.in_scope(&listing()))
			.await?
			.into_iter()
			.map(crate::domain::run_state::RawRun::inspect)
			.collect(),
			human_requests: human,
			conversations,
			events,
			artifacts,
		};
		crate::authorization::remote::deployment_administrator::filter_state(
			&mut f.store.pool.begin(&listing()).await?,
			&mut state,
		)
		.await?;
		Ok(Response::ok().with_json(&state)?)
	}
	/// The OpenRouter catalog for model definitions, which any member may
	/// read, from one process-wide copy (#362).
	pub(crate) async fn openrouter_models(
		&self,
	) -> Result<aidash_domain::catalog::CatalogSnapshot> {
		static CATALOG: aidash_application::catalog::CatalogCache =
			aidash_application::catalog::CatalogCache::new();
		Ok(CATALOG.get(self.catalog.as_ref(), chrono::Utc::now).await?)
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
		if let Actor::Subject(identity) = actor {
			let mut details = execution::details_page(&f, &identity, id, page.offset).await?;
			details.media_input_routes = f.run_media_input_routes(&details.run).await?;
			return Ok(details);
		}
		let scope = f.store.run_tenancy(id).await?;
		let run = f.store.inspect_run(&scope, id).await?;
		crate::authorization::remote::deployment_administrator::require(
			&mut f.store.pool.begin(&scope).await?,
			run.workspace_id,
		)
		.await?;
		let mut details = f.store.run_details(&scope, id, page.offset).await?;
		details.media_input_routes = f.run_media_input_routes(&details.run).await?;
		Ok(details)
	}
	/// One Source citation of a Run, for a reader who may read that Run now.
	/// Every reason the reader cannot see it is the same `NotFound`.
	pub(crate) async fn run_evidence(
		&self,
		actor: Actor,
		run: Uuid,
		reference: &str,
	) -> Result<CitedEvidence> {
		let f = self.runtime.clone();
		let readers = crate::apps::execution::repositories::research::Readers {
			federation: &f,
			actor: &actor,
		};
		let scope = match &actor {
			Actor::Subject(identity) => identity.tenancy()?,
			Actor::DeploymentAdministrator => f.store.run_tenancy(run).await?,
		};
		let store = crate::research::evidence_store(f.store.clone(), scope);
		Ok(
			aidash_application::research::reading::read(&readers, &store, run, reference)
				.await?
				.into(),
		)
	}
	/// Revoke all of one Run's Research evidence (ADR 0046).
	pub(crate) async fn research_revocation(
		&self,
		actor: Actor,
		browser: Option<crate::dashboard_auth::BrowserOrigin>,
		run: Uuid,
		input: RevocationInput,
	) -> Result<RevocationRecord> {
		let f = self.runtime.clone();
		let revocations = Revocations {
			federation: &f,
			actor: &actor,
			administrator: administrator(&actor, browser),
		};
		if let Some(tenant) = &input.tenant {
			aidash_domain::policy::identifier(tenant)?;
		}
		Ok(aidash_application::research::revocation::revoke(
			&revocations,
			run,
			&input.reason,
			input.tenant.as_deref(),
			chrono::Utc::now(),
		)
		.await?
		.into())
	}
	/// The revocation record of one Run, for the audience of its Tenant's
	/// records.
	pub(crate) async fn research_revocation_record(
		&self,
		actor: Actor,
		browser: Option<crate::dashboard_auth::BrowserOrigin>,
		run: Uuid,
	) -> Result<RevocationRecord> {
		let f = self.runtime.clone();
		let revocations = Revocations {
			federation: &f,
			actor: &actor,
			administrator: administrator(&actor, browser),
		};
		Ok(
			aidash_application::research::revocation::record(&revocations, run)
				.await?
				.into(),
		)
	}
	/// One Tenant's Evidence Revocation records, newest first.
	pub(crate) async fn research_revocations(
		&self,
		actor: Actor,
		browser: Option<crate::dashboard_auth::BrowserOrigin>,
		tenant: String,
		page: HistoryPage,
	) -> Result<Vec<RevocationRecord>> {
		aidash_domain::policy::identifier(&tenant)?;
		let f = self.runtime.clone();
		let revocations = Revocations {
			federation: &f,
			actor: &actor,
			administrator: administrator(&actor, browser),
		};
		Ok(aidash_application::research::revocation::records(
			&revocations,
			&tenant,
			page.offset,
			page.limit,
		)
		.await?
		.into_iter()
		.map(Into::into)
		.collect())
	}
	/// The Retention Rules that apply to one Tenant (ADR 0045).
	pub(crate) async fn retention(
		&self,
		actor: Actor,
		browser: Option<crate::dashboard_auth::BrowserOrigin>,
		tenant: String,
	) -> Result<TenantRetentionView> {
		aidash_domain::policy::identifier(&tenant)?;
		let administrator = administrator(&actor, browser);
		let repository = self.retention_repository(&actor, administrator)?;
		let retention = aidash_application::retention::rules(&repository, &tenant).await?;
		self.retention_view(&actor, administrator, retention).await
	}
	/// What the caller may do about one Tenant's periods and its revocation
	/// records, beside the periods themselves.
	async fn retention_view(
		&self,
		actor: &Actor,
		administrator: Option<Uuid>,
		retention: aidash_application::retention::TenantRetention,
	) -> Result<TenantRetentionView> {
		let repository = self.retention_repository(actor, administrator)?;
		let can_update =
			aidash_application::retention::may_update(&repository, &retention.tenant).await?;
		let can_read_revocations = self
			.may_read_revocations(actor, administrator, &retention.tenant)
			.await?;
		Ok(TenantRetentionView {
			retention,
			can_update,
			// The same permission changes the periods and reads their history.
			can_read_history: can_update,
			can_read_revocations,
		})
	}
	async fn may_read_revocations(
		&self,
		actor: &Actor,
		administrator: Option<Uuid>,
		tenant: &str,
	) -> Result<bool> {
		let f = self.runtime.clone();
		let revocations = Revocations {
			federation: &f,
			actor,
			administrator,
		};
		Ok(
			aidash_application::research::revocation::may_read_records(&revocations, tenant)
				.await?,
		)
	}
	/// Store or clear one Tenant's own, shorter periods.
	pub(crate) async fn retention_update(
		&self,
		actor: Actor,
		browser: Option<crate::dashboard_auth::BrowserOrigin>,
		tenant: String,
		input: RetentionInput,
	) -> Result<TenantRetentionView> {
		aidash_domain::policy::identifier(&tenant)?;
		let administrator = administrator(&actor, browser);
		let repository = self.retention_repository(&actor, administrator)?;
		// Nothing fallible may follow the commit, or a committed change would
		// answer as an error and a retry would record it twice. The flags
		// are therefore read first; the update itself proves the caller may
		// update the periods and read their history.
		let can_read_revocations = self
			.may_read_revocations(&actor, administrator, &tenant)
			.await?;
		let retention =
			aidash_application::retention::update(&repository, &tenant, input.rules).await?;
		Ok(TenantRetentionView {
			retention,
			can_update: true,
			can_read_history: true,
			can_read_revocations,
		})
	}
	/// The history of one Tenant's periods, newest first.
	pub(crate) async fn retention_history(
		&self,
		actor: Actor,
		browser: Option<crate::dashboard_auth::BrowserOrigin>,
		tenant: String,
		page: HistoryPage,
	) -> Result<Vec<aidash_domain::retention::RetentionChange>> {
		aidash_domain::policy::identifier(&tenant)?;
		let repository = self.retention_repository(&actor, administrator(&actor, browser))?;
		Ok(
			aidash_application::retention::history(&repository, &tenant, page.offset, page.limit)
				.await?,
		)
	}
	fn retention_repository<'a>(
		&'a self,
		actor: &'a Actor,
		administrator: Option<Uuid>,
	) -> Result<TenantRetention<'a>> {
		Ok(TenantRetention {
			store: &self.runtime.store,
			actor,
			administrator,
			deployment: self.runtime.store.research_retention,
		})
	}
	pub(crate) async fn run_control(
		&self,
		actor: Actor,
		browser: Option<crate::dashboard_auth::BrowserOrigin>,
		id: Uuid,
		input: ControlInput,
	) -> Result<crate::domain::RunInspection> {
		let f = self.runtime.clone();
		if matches!(actor, Actor::DeploymentAdministrator)
			&& browser.is_some()
			&& input.action == crate::domain::RunControlAction::Resume
		{
			return Err(Error::Forbidden);
		}
		if let Actor::Subject(identity) = actor {
			return execution::control(&f, &identity, id, input.action).await;
		}
		let scope = f.store.run_tenancy(id).await?;
		let existing = f.store.inspect_run(&scope, id).await?;
		let mut tx = f.store.pool.begin(&scope).await?;
		crate::authorization::remote::deployment_administrator::require(
			&mut tx,
			existing.workspace_id,
		)
		.await?;
		if input.action == crate::domain::RunControlAction::Resume
			&& f.store
				.pool
				.dashboard_policy()
				.is_some_and(|policy| policy.authority.is_some())
			&& crate::apps::identity::models::DashboardExecutionOrigin::is_human_origin(
				&mut *tx, id,
			)
			.await?
		{
			// Common-auth human work is readmitted only by a current interactive
			// session of its original source, through the Subject control path.
			return Err(Error::Forbidden);
		}
		let r = f.store.control_in(&mut tx, id, input.action).await?;
		tx.commit().await?;
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
		let mut connection = f.store.pool.begin(&listing()).await?;
		if let Some(workspace) = q.workspace_id
			&& !crate::authorization::remote::deployment_administrator::visible(
				&mut connection,
				workspace,
			)
			.await?
		{
			let mut response = Response::ok().with_json(&visible)?;
			response
				.headers
				.insert("x-aidash-event-cursor", cursor.into());
			return Ok(response);
		}
		loop {
			let page = f
				.store
				.events(&listing(), cursor, q.workspace_id, 500)
				.await?;
			let exhausted = page.len() < 500;
			if let Some(last) = page.last() {
				cursor = last.sequence;
			}
			visible.extend(
				crate::authorization::remote::deployment_administrator::filter_events(
					&mut connection,
					page,
				)
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
		let stream = self
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
			let model: crate::registry::ModelConfig = serde_json::from_value(model.config)?;
			model.require_media_types(
				attachments
					.iter()
					.map(|attachment| attachment.media_type.as_str()),
			)?;
			let headroom = f.run_request_headroom(&run).await?;
			let version = agent.projection_version.unwrap_or_default();
			if !agent.harness_contract.unwrap_or_default().supports(version) {
				return Err(Error::Invalid(format!(
					"Harness Contract {} does not support Projection Version {version}",
					agent.harness_contract.unwrap_or_default()
				)));
			}
			let request = aidash_application::execution::media::admission_request(
				&model,
				version,
				json!({"run_message":input.content}),
				parts,
			);
			request.validate()?;
			crate::generation::budget::Reservation::check_request(headroom, &request)?;
			Ok(())
		}
		.await;
		lease.finish(result).await?;
		f.notify.notify_waiters();
		Ok(SentResponse { sent: true })
	}
	/// A Run's research usage, limits and estimated cost, for a reader who
	/// may read that Run now, decided exactly as [`Self::run_get`] decides it.
	pub(crate) async fn run_research(
		&self,
		actor: Actor,
		id: Uuid,
	) -> Result<crate::apps::execution::serializers::runs::RunResearch> {
		let f = self.runtime.clone();
		let scope = match &actor {
			Actor::Subject(identity) => identity.tenancy()?,
			Actor::DeploymentAdministrator => f.store.run_tenancy(id).await?,
		};
		let readable = || async {
			if let Actor::Subject(identity) = &actor {
				execution::details_page(&f, identity, id, 0).await?;
			} else {
				let run = f.store.inspect_run(&scope, id).await?;
				crate::authorization::remote::deployment_administrator::require(
					&mut f.store.pool.begin(&scope).await?,
					run.workspace_id,
				)
				.await?;
			}
			Ok::<_, crate::Error>(())
		};
		readable().await?;
		let research = crate::apps::execution::repositories::research::run_research(
			&f.store,
			&scope,
			id,
			crate::research::providers().limits,
		)
		.await?;
		let revocations = Revocations {
			federation: &f,
			actor: &actor,
			administrator: None,
		};
		let standing = aidash_application::research::revocation::standing(&revocations, id).await?;
		// The research reads are separate transactions: authority revoked
		// meanwhile withholds them, as for a cited fragment.
		readable().await?;
		Ok(crate::apps::execution::serializers::runs::RunResearch {
			revoked_at: standing.revoked_at,
			can_revoke: standing.may_revoke,
			can_read_revocation: standing.may_read_record,
			..research
		})
	}
	/// The Research providers of this Deployment for a Tenant's subject.
	pub(crate) async fn research_providers(
		&self,
		actor: Actor,
		input: crate::apps::execution::serializers::runs::ResearchProvidersQuery,
	) -> Result<crate::apps::execution::serializers::runs::ResearchProviders> {
		crate::apps::execution::capabilities::services::endpoints::research_providers(
			self.runtime.clone(),
			actor,
			input,
		)
		.await
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
