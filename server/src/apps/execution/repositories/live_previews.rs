//! A Live preview's persistence, Runner dispatch and lifecycle stops (ADR 0070).
//!
//! A start takes the holds over the revision, records a `starting` row and then
//! dispatches the Runner's long-lived `serve` operation outside any authority
//! transaction, because waiting for a port must not hold one. Stops settle a
//! row and release its holds only after the Runner proves the Pod is gone.
use crate::apps::federation::peer::models::AuthorizationRemoteAdmission;
use crate::{
	Error as NativeError, Result as NativeResult,
	apps::execution::capabilities::{
		models::{CoreLivePreviews, CorePreviewLimits},
		services::retention,
	},
	authorization::{access::Access, identity::SubjectIdentity},
	domain::Run,
	store::Store,
};
use aidash_application::preview::live::{self as application, RunFacts, Start, Started, Status};
use aidash_application::tenancy::TenancyScope;
use aidash_domain::capabilities::retention::Holder;
use aidash_domain::preview::live::{Limits, LivePreview, State, StopReason};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::db::orm::Model;
use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};
use uuid::Uuid;

type Row = CoreLivePreviews;

pub(crate) fn domain(row: Row) -> NativeResult<LivePreview> {
	let port = u16::try_from(row.port).map_err(|_| NativeError::Invalid("invalid port".into()))?;
	Ok(LivePreview {
		id: row.id,
		tenant: row.tenant,
		run_id: row.run_id,
		area_id: row.area_id,
		revision: row.revision,
		port,
		state: State::parse(&row.state)?,
		stop_reason: row
			.stop_reason
			.as_deref()
			.map(StopReason::parse)
			.transpose()?,
		limits: Limits {
			idle_seconds: row.idle_seconds,
			absolute_seconds: row.absolute_seconds,
		},
		started_at: row.started_at,
		last_request_at: row.last_request_at,
		expires_at: row.expires_at,
		pod_host: row.pod_host,
		termination_confirmed: row.termination_confirmed,
	})
}

pub(crate) fn to_row(preview: &LivePreview) -> Row {
	Row::build()
		.id(preview.id)
		.tenant(preview.tenant.clone())
		.run_id(preview.run_id)
		.area_id(preview.area_id)
		.revision(preview.revision)
		.port(i32::from(preview.port))
		.state(preview.state.as_str().to_owned())
		.stop_reason(preview.stop_reason.map(|reason| reason.as_str().to_owned()))
		.idle_seconds(preview.limits.idle_seconds)
		.absolute_seconds(preview.limits.absolute_seconds)
		.started_at(preview.started_at)
		.last_request_at(preview.last_request_at)
		.expires_at(preview.expires_at)
		.pod_host(preview.pod_host.clone())
		.termination_confirmed(preview.termination_confirmed)
		.updated_at(Utc::now())
		.finish()
}

/// The application's authority, persistence and Runner boundary.
pub(crate) struct Scope<'a> {
	pub access: &'a mut Access,
	pub store: &'a Store,
	pub run: Run,
	pub domain: Option<String>,
}

impl Scope<'_> {
	async fn load(&mut self, id: Uuid) -> NativeResult<Option<LivePreview>> {
		let row = Row::objects()
			.filter(Row::field_id().eq(id))
			.all_with_executor(&mut **self.access.tx)
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(NativeError::from)?
			.pop();
		row.map(domain).transpose()
	}
}

#[async_trait]
impl application::Scope for Scope<'_> {
	fn domain(&self) -> Option<&str> {
		self.domain.as_deref()
	}
	fn ceilings(&self) -> Limits {
		self.store.preview_limits.clone()
	}
	async fn tenant_limits(&mut self, tenant: &str) -> aidash_application::Result<Option<Limits>> {
		let row = CorePreviewLimits::objects()
			.filter(CorePreviewLimits::field_tenant().eq(tenant))
			.all_with_executor(&mut **self.access.tx)
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(NativeError::from)?
			.pop();
		Ok(row.map(|row| Limits {
			idle_seconds: row.idle_seconds,
			absolute_seconds: row.absolute_seconds,
		}))
	}
	async fn run(&mut self) -> aidash_application::Result<RunFacts> {
		let area = aidash_application::capabilities::sessions::for_run(
			&mut crate::bootstrap::session_scope(None, self.access),
			&self.run.metadata(),
		)
		.await
		.map_err(|error| {
			let message = error.to_string();
			let native = NativeError::from(error);
			if native.is_database() {
				native
			} else {
				NativeError::External(message)
			}
		})?;
		Ok(RunFacts {
			id: self.run.id,
			tenant: self.access.identity.tenant.clone(),
			area_id: area.id,
		})
	}
	async fn area(
		&mut self,
	) -> aidash_application::Result<aidash_domain::capabilities::sessions::Area> {
		aidash_application::capabilities::sessions::for_run(
			&mut crate::bootstrap::session_scope(None, self.access),
			&self.run.metadata(),
		)
		.await
	}
	async fn current(&mut self, run: Uuid) -> aidash_application::Result<Option<LivePreview>> {
		let rows = Row::objects()
			.filter(Row::field_run_id().eq(run))
			.all_with_executor(&mut **self.access.tx)
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(NativeError::from)?;
		let row = rows.into_iter().max_by_key(|row| row.started_at);
		row.map(domain).transpose().map_err(Into::into)
	}
	async fn by_id(&mut self, id: Uuid) -> aidash_application::Result<Option<LivePreview>> {
		self.load(id).await.map_err(Into::into)
	}
	async fn insert(&mut self, preview: &LivePreview) -> aidash_application::Result<()> {
		Row::objects()
			.insert_with_executor(&mut **self.access.tx, &to_row(preview))
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(NativeError::from)?;
		Ok(())
	}
	async fn save(&mut self, preview: &LivePreview) -> aidash_application::Result<()> {
		Row::objects()
			.save_with_executor(&mut **self.access.tx, &to_row(preview))
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(NativeError::from)?;
		Ok(())
	}
	async fn retain_revision(
		&mut self,
		area_id: Uuid,
		revision: i64,
		holder: &Holder,
	) -> aidash_application::Result<()> {
		let hold =
			retention::retain_revision(self.store, self.access, area_id, revision, holder.clone())
				.await?;
		debug_assert_eq!(hold.revision, revision);
		Ok(())
	}
	async fn release(&mut self, holder: &Holder) -> aidash_application::Result<()> {
		retention::release(self.store, self.access, holder)
			.await
			.map(drop)
			.map_err(Into::into)
	}
	async fn unsettled(&mut self) -> aidash_application::Result<Vec<LivePreview>> {
		let rows = Row::objects()
			.filter(Row::field_id().ne(Uuid::nil()))
			.all_with_executor(&mut **self.access.tx)
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(NativeError::from)?;
		rows.into_iter()
			.map(domain)
			.filter(|row| {
				row.as_ref()
					.is_ok_and(|preview| !preview.state.is_stopped() || preview.needs_termination())
			})
			.collect::<NativeResult<Vec<_>>>()
			.map_err(Into::into)
	}
	async fn area_revision(&mut self, area_id: Uuid) -> aidash_application::Result<Option<i64>> {
		let revision: Option<i64> = crate::database::native::query_scalar(
			&reinhardt::query::Query::select()
				.column(reinhardt::query::Alias::new("revision"))
				.from(reinhardt::query::Alias::new("core_areas"))
				.and_where(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("id"))
						.eq(reinhardt::query::Expr::value(area_id)),
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.scalar_optional(&mut **self.access.tx)
		.await?;
		Ok(revision)
	}
}

/// The start row, before the Runner is dispatched.
pub(crate) async fn prepare(
	store: &Store,
	access: &mut Access,
	run: &Run,
	domain: Option<String>,
	input: Start,
) -> NativeResult<LivePreview> {
	let mut scope = Scope {
		access,
		store,
		run: run.clone(),
		domain,
	};
	application::start(&mut scope, &input)
		.await
		.map_err(Into::into)
}

/// The Runner's answer to a dispatch.
pub(crate) struct Dispatch {
	pub pod_host: Option<String>,
	pub startup_log: String,
	pub started_at: DateTime<Utc>,
	pub failure: Option<String>,
}

impl Dispatch {
	/// A dispatch that never reached the Runner still releases the revision's
	/// holds through the recorded `start_failed` stop.
	pub fn failed(message: impl Into<String>) -> Self {
		Self {
			pod_host: None,
			startup_log: String::new(),
			started_at: Utc::now(),
			failure: Some(message.into()),
		}
	}
}

/// Record the Runner's answer: serving on success, or start-failed after the
/// revision's holds are released.
pub(crate) async fn finish_start(
	store: &Store,
	access: &mut Access,
	run: &Run,
	domain: Option<String>,
	pending: LivePreview,
	dispatched: Dispatch,
) -> NativeResult<Started> {
	let mut scope = Scope {
		access,
		store,
		run: run.clone(),
		domain,
	};
	match dispatched.pod_host {
		Some(pod_host) if dispatched.failure.is_none() => application::serving(
			&mut scope,
			pending,
			pod_host,
			&dispatched.startup_log,
			dispatched.started_at,
		)
		.await
		.map_err(Into::into),
		_ => application::failed(
			&mut scope,
			pending,
			StopReason::StartFailed,
			&dispatched.startup_log,
		)
		.await
		.map_err(Into::into),
	}
}

/// The Run's Live preview with staleness resolved.
pub(crate) async fn status(
	store: &Store,
	access: &mut Access,
	run: &Run,
	domain: Option<String>,
) -> NativeResult<Option<Status>> {
	let mut scope = Scope {
		access,
		store,
		run: run.clone(),
		domain,
	};
	application::status(&mut scope, run.id)
		.await
		.map_err(Into::into)
}

/// Begin a stop in the caller's transaction.
pub(crate) async fn request_stop(
	store: &Store,
	access: &mut Access,
	run: &Run,
	domain: Option<String>,
	id: Uuid,
	reason: StopReason,
) -> NativeResult<LivePreview> {
	let mut scope = Scope {
		access,
		store,
		run: run.clone(),
		domain,
	};
	application::stop(&mut scope, id, reason)
		.await
		.map_err(Into::into)
}

/// Settle a stopped row and release its holds after the Pod proof.
pub(crate) async fn settle(
	store: &Store,
	access: &mut Access,
	run: &Run,
	domain: Option<String>,
	id: Uuid,
) -> NativeResult<Option<LivePreview>> {
	let mut scope = Scope {
		access,
		store,
		run: run.clone(),
		domain,
	};
	application::settled(&mut scope, id)
		.await
		.map_err(Into::into)
}

/// Ask the Runner to stop the Pod and wait for its termination proof.
pub(crate) async fn terminate(store: &Store, preview: &LivePreview) -> NativeResult<bool> {
	// A Live preview is a Background process of its Run in the Execution Pod (#745).
	super::operations::pods::stop_preview(store, preview.id).await
}

/// Mark every unsettled Live preview of `run` as stopping inside the caller's
/// transaction. A cancellation hook calls this so no Pod outlives the Run.
pub(crate) async fn mark_stopped_for_run(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	run: Uuid,
	reason: StopReason,
) -> NativeResult<()> {
	use reinhardt::query::{Alias, Expr};
	mark_stopped(
		tx,
		reason,
		Expr::col(Alias::new("run_id")).eq(Expr::value(run)),
	)
	.await
}

/// Mark every unsettled Live preview of `tenant` as stopping.
pub(crate) async fn mark_stopped_for_tenant(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	tenant: &str,
	reason: StopReason,
) -> NativeResult<()> {
	use reinhardt::query::{Alias, Expr};
	mark_stopped(
		tx,
		reason,
		Expr::col(Alias::new("tenant")).eq(Expr::value(tenant.to_owned())),
	)
	.await
}

/// Mark every unsettled Live preview matching `filter` as stopping. The
/// sweep then asks the Runner and settles each row only on its proof.
pub(crate) async fn mark_stopped(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	reason: StopReason,
	filter: reinhardt::query::SimpleExpr,
) -> NativeResult<()> {
	use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
	let (sql, values) = Query::update()
		.table(Alias::new("core_live_previews"))
		.value_expr(Alias::new("state"), Expr::value("stopped"))
		.value_expr(Alias::new("stop_reason"), Expr::value(reason.as_str()))
		.value_expr(Alias::new("termination_confirmed"), Expr::value(false))
		.value_expr(Alias::new("updated_at"), Expr::cust("CURRENT_TIMESTAMP"))
		.and_where(Expr::col(Alias::new("state")).ne("stopped"))
		.and_where(filter)
		.build(PostgresQueryBuilder);
	tx.execute(&sql, reinhardt::db::orm::execution::convert_values(values))
		.await
		.map_err(NativeError::from)?;
	Ok(())
}

/// The Live previews a Read Recheck covers.
pub(crate) async fn serving(store: &Store) -> NativeResult<Vec<Uuid>> {
	let mut tx = store
		.pool
		.begin(&TenancyScope::Deployment(
			aidash_application::tenancy::DeploymentScopeReason::CapabilityMaintenance,
		))
		.await?;
	let mut ids = vec![];
	for state in [State::Running, State::Stale] {
		let rows = Row::objects()
			.filter(Row::field_state().eq(state.as_str()))
			.all_with_executor(&mut *tx)
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(NativeError::from)?;
		ids.extend(rows.into_iter().map(|row| row.id));
	}
	tx.commit().await?;
	Ok(ids)
}

async fn load_for_maintenance(store: &Store, id: Uuid) -> NativeResult<Option<LivePreview>> {
	let mut tx = store
		.pool
		.begin(&TenancyScope::Deployment(
			aidash_application::tenancy::DeploymentScopeReason::CapabilityMaintenance,
		))
		.await?;
	let row = Row::objects()
		.filter(Row::field_id().eq(id))
		.all_with_executor(&mut *tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)
		.map_err(NativeError::from)?
		.pop();
	tx.commit().await?;
	row.map(domain).transpose()
}

/// The Read Recheck of a serving Live preview, for everyone on its Run's chain
/// (ADR 0070, ADR 0116). A Run that has finished stays covered: its grant remains.
pub(crate) async fn read_recheck(store: &Store, id: Uuid) -> NativeResult<bool> {
	use aidash_application::ports::authorization::ExecutionGrantRepository as _;
	let Some(preview) = load_for_maintenance(store, id).await? else {
		return Ok(true);
	};
	let grant = crate::bootstrap::execution_grants(store)
		.grant(preview.run_id)
		.await?;
	let (identity, subjects) = match grant {
		Some(grant) => (
			SubjectIdentity {
				http_session: None,
				credential_id: grant.credential_id,
				tenant: grant.tenant,
				subject: grant.root_subject,
			},
			Some(grant.subject_chain),
		),
		// A Run started through a Remote Grant has no local grant. Its admission
		// names the credential and the subject chain that hold the preview.
		None => {
			let mut tx = store
				.pool
				.begin(&TenancyScope::tenant(&preview.tenant)?)
				.await?;
			let admission = AuthorizationRemoteAdmission::objects()
				.filter(AuthorizationRemoteAdmission::field_id().eq(preview.run_id))
				.filter(AuthorizationRemoteAdmission::field_tenant().eq(preview.tenant.clone()))
				.all_with_executor(&mut *tx)
				.await
				.map_err(reinhardt::core::exception::Error::from)
				.map_err(NativeError::from)?
				.pop();
			tx.commit().await?;
			let Some(admission) = admission else {
				return Ok(false);
			};
			let Some(person) = admission.subject_chain.first().cloned() else {
				return Ok(false);
			};
			(
				SubjectIdentity {
					http_session: None,
					credential_id: admission.credential_id,
					tenant: admission.tenant,
					subject: person,
				},
				Some(admission.subject_chain),
			)
		}
	};
	let mut access = Access::begin(store, &identity).await?;
	if let Some(subjects) = subjects {
		access.subjects = subjects;
	}
	let result = access.recheck_run_reads(preview.run_id).await.map(|_| ());
	access.finish(result).await.map(|()| true)
}

/// Stop a serving Live preview for a Read Recheck's reason. As in the sweep, the
/// row settles when the Runner proves the Pod is gone, here or on a later pass.
pub(crate) async fn stop_serving(store: &Store, id: Uuid, reason: StopReason) -> NativeResult<()> {
	let Some(mut preview) = load_for_maintenance(store, id).await? else {
		return Ok(());
	};
	if !matches!(preview.state, State::Running | State::Stale) {
		return Ok(());
	}
	let tenant = preview.tenant.clone();
	preview.stopping(reason);
	{
		let mut tx = store.pool.begin(&TenancyScope::tenant(&tenant)?).await?;
		Row::objects()
			.save_with_executor(&mut *tx, &to_row(&preview))
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(NativeError::from)?;
		tx.commit().await?;
	}
	if terminate(store, &preview).await.unwrap_or(false) {
		preview.termination_confirmed = true;
		settle_row(store, &tenant, &mut preview).await?;
	}
	Ok(())
}

/// One sweep pass: stale marking and stops at the idle, absolute and start
/// limits. Each stop asks the Runner and settles the row only on its proof.
pub(crate) async fn sweep(store: &Store) -> NativeResult<()> {
	let now = Utc::now();
	let rows = {
		let mut tx = store
			.pool
			.begin(&TenancyScope::Deployment(
				aidash_application::tenancy::DeploymentScopeReason::CapabilityMaintenance,
			))
			.await?;
		let rows = Row::objects()
			.filter(Row::field_id().ne(Uuid::nil()))
			.all_with_executor(&mut *tx)
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(NativeError::from)?;
		tx.commit().await?;
		rows
	};
	for stored in rows {
		let tenant = stored.tenant.clone();
		let mut preview = match domain(stored) {
			Ok(preview) => preview,
			Err(_) => continue,
		};
		let Some(reason) = preview.expired(now).or_else(|| {
			preview
				.start_expired(now)
				.then_some(StopReason::StartFailed)
		}) else {
			if preview.state == State::Running {
				let mut tx = store.pool.begin(&TenancyScope::tenant(&tenant)?).await?;
				if let Some(revision) = area_revision(&mut tx, preview.area_id).await?
					&& revision > preview.revision
				{
					preview.state = State::Stale;
					Row::objects()
						.save_with_executor(&mut *tx, &to_row(&preview))
						.await
						.map_err(reinhardt::core::exception::Error::from)
						.map_err(NativeError::from)?;
				}
				tx.commit().await?;
			}
			continue;
		};
		preview.stopping(reason);
		{
			let mut tx = store.pool.begin(&TenancyScope::tenant(&tenant)?).await?;
			Row::objects()
				.save_with_executor(&mut *tx, &to_row(&preview))
				.await
				.map_err(reinhardt::core::exception::Error::from)
				.map_err(NativeError::from)?;
			tx.commit().await?;
		}
		if terminate(store, &preview).await.unwrap_or(false) {
			settle_row(store, &tenant, &mut preview).await?;
		}
	}
	// Rows whose Pod proof arrived late, and stops a cancellation hook asked for.
	sweep_stopping(store).await
}

async fn sweep_stopping(store: &Store) -> NativeResult<()> {
	let rows = {
		let mut tx = store
			.pool
			.begin(&TenancyScope::Deployment(
				aidash_application::tenancy::DeploymentScopeReason::CapabilityMaintenance,
			))
			.await?;
		let rows = Row::objects()
			.filter(Row::field_state().eq("stopped"))
			.filter(Row::field_termination_confirmed().eq(false))
			.all_with_executor(&mut *tx)
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(NativeError::from)?;
		tx.commit().await?;
		rows
	};
	for stored in rows {
		let tenant = stored.tenant.clone();
		let Ok(mut preview) = domain(stored) else {
			continue;
		};
		if !terminate(store, &preview).await.unwrap_or(false) {
			continue;
		}
		preview.termination_confirmed = true;
		settle_row(store, &tenant, &mut preview).await?;
	}
	Ok(())
}

async fn settle_row(store: &Store, tenant: &str, preview: &mut LivePreview) -> NativeResult<()> {
	let mut tx = store.pool.begin(&TenancyScope::tenant(tenant)?).await?;
	Row::objects()
		.save_with_executor(&mut *tx, &to_row(preview))
		.await
		.map_err(reinhardt::core::exception::Error::from)
		.map_err(NativeError::from)?;
	release_hold(&mut tx, tenant, preview.id).await?;
	tx.commit().await?;
	Ok(())
}

async fn area_revision(
	tx: &mut crate::database::native::Transaction,
	area: Uuid,
) -> NativeResult<Option<i64>> {
	crate::database::native::query_scalar(
		&reinhardt::query::Query::select()
			.column(reinhardt::query::Alias::new("revision"))
			.from(reinhardt::query::Alias::new("core_areas"))
			.and_where(
				reinhardt::query::Expr::col(reinhardt::query::Alias::new("id"))
					.eq(reinhardt::query::Expr::value(area)),
			)
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.scalar_optional(&mut **tx)
	.await
}

async fn release_hold(
	tx: &mut crate::database::native::Transaction,
	tenant: &str,
	preview: Uuid,
) -> NativeResult<()> {
	let holder = Holder::LivePreview {
		live_preview_id: preview,
	};
	crate::database::native::query(
		&reinhardt::query::Query::delete()
			.from_table(reinhardt::query::Alias::new("core_retention_holds"))
			.and_where(
				reinhardt::query::Expr::col(reinhardt::query::Alias::new("tenant"))
					.eq(reinhardt::query::Expr::value(tenant.to_owned())),
			)
			.and_where(
				reinhardt::query::Expr::col(reinhardt::query::Alias::new("holder_kind"))
					.eq(reinhardt::query::Expr::value(holder.kind())),
			)
			.and_where(
				reinhardt::query::Expr::col(reinhardt::query::Alias::new("holder_id"))
					.eq(reinhardt::query::Expr::value(holder.id())),
			)
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.execute(&mut **tx)
	.await?;
	Ok(())
}

#[cfg(test)]
mod database_failure_tests {
	use super::*;
	use crate::apps::execution::test_database::{DatabaseFixture, database};

	#[rstest::rstest]
	#[tokio::test]
	async fn live_preview_repository_preserves_real_pool_timeout(
		#[future] database: DatabaseFixture,
	) {
		let database = database.await;
		let driver = sqlx::postgres::PgPoolOptions::new()
			.max_connections(8)
			.acquire_timeout(std::time::Duration::from_secs(2))
			.connect(&database.url)
			.await
			.unwrap();
		let store = Store::from_pool(
			driver.clone(),
			aidash_domain::nodes::DeploymentId::parse("aidash://preview-test").unwrap(),
		)
		.await
		.unwrap();
		let mut connections = Vec::new();
		for _ in 0..8 {
			connections.push(driver.acquire().await.unwrap());
		}
		assert!(matches!(
			driver.acquire().await.err(),
			Some(sqlx::Error::PoolTimedOut)
		));
		let error = serving(&store).await.unwrap_err();
		assert!(matches!(
			crate::apps::execution::repositories::worker::classify_failure(&error),
			aidash_application::recovery::ExecutionFailure::DatabaseUnavailable
		));
		let application = aidash_application::Error::from(error);
		assert!(matches!(
			application,
			aidash_application::Error::TemporaryDatabase(_)
		));
		let response = NativeError::from(application).http_response();
		assert_eq!(response.status.as_u16(), 503);
		assert_eq!(response.headers[http::header::RETRY_AFTER], "1");
	}

	#[rstest::rstest]
	#[tokio::test]
	async fn live_preview_repository_preserves_orm_statement_timeout(
		#[future] database: DatabaseFixture,
	) {
		let database = database.await;
		let options: sqlx::postgres::PgConnectOptions = database.url.parse().unwrap();
		let driver = sqlx::postgres::PgPoolOptions::new()
			.max_connections(8)
			.connect_with(options.options([("statement_timeout", "2s")]))
			.await
			.unwrap();
		let store = Store::from_pool(
			driver,
			aidash_domain::nodes::DeploymentId::parse("aidash://preview-test").unwrap(),
		)
		.await
		.unwrap();
		let owner = sqlx::PgPool::connect(&database.owner_url).await.unwrap();
		let mut lock = owner.begin().await.unwrap();
		sqlx::query("LOCK TABLE core_live_previews IN ACCESS EXCLUSIVE MODE")
			.execute(&mut *lock)
			.await
			.unwrap();
		let error = serving(&store).await.unwrap_err();
		assert!(error.has_database_code("57014"), "{error:?}");
		assert!(error.is_transient_database(), "{error:?}");
		assert!(matches!(
			aidash_application::Error::from(error),
			aidash_application::Error::TemporaryDatabase(_)
		));
		lock.rollback().await.unwrap();
	}
}
