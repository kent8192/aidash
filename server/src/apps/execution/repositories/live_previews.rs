//! A Live preview's persistence, Runner dispatch and lifecycle stops (ADR 0070).
//!
//! A start takes the holds over the revision, records a `starting` row and then
//! dispatches the Runner's long-lived `serve` operation outside any authority
//! transaction, because waiting for a port must not hold one. Stops settle a
//! row and release its holds only after the Runner proves the Pod is gone.
use crate::{
	Error as NativeError, Result as NativeResult,
	apps::execution::capabilities::{
		models::{CoreLivePreviews, CorePreviewLimits, EgressSession},
		serializers::contracts::FileEntry,
		services::retention,
	},
	authorization::access::Access,
	domain::Run,
	store::Store,
};
use aidash_application::preview::live::{self as application, RunFacts, Start, Started, Status};
use aidash_application::tenancy::TenancyScope;
use aidash_domain::capabilities::retention::Holder;
use aidash_domain::preview::live::{Limits, LivePreview, START_SECONDS, State, StopReason};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::db::orm::Model;
use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};
use sha2::Digest as _;
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
		egress_expires_at: row.egress_expires_at,
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
		.egress_expires_at(preview.egress_expires_at)
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
			.map_err(|error| NativeError::External(error.to_string()))?
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
			.map_err(|error| NativeError::External(error.to_string()))?
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
		.map_err(|error| NativeError::External(error.to_string()))?;
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
			.map_err(|error| NativeError::External(error.to_string()))?;
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
			.map_err(|error| NativeError::External(error.to_string()))?;
		Ok(())
	}
	async fn save(&mut self, preview: &LivePreview) -> aidash_application::Result<()> {
		Row::objects()
			.save_with_executor(&mut **self.access.tx, &to_row(preview))
			.await
			.map_err(|error| NativeError::External(error.to_string()))?;
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
			.map_err(|error| NativeError::External(error.to_string()))?;
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
		.await
		.map_err(|error| NativeError::External(error.to_string()))?;
		Ok(revision)
	}
	async fn mint_egress(&mut self, preview: &mut LivePreview) -> aidash_application::Result<()> {
		let Some(settings) = self.store.capabilities.0.egress.as_ref() else {
			return Ok(());
		};
		let expires_at = preview.expires_at;
		EgressSession::objects()
			.insert_with_executor(
				&mut **self.access.tx,
				&EgressSession::build()
					.id(preview.id)
					.tenant(self.access.identity.tenant.clone())
					.run_id(preview.run_id)
					.live_preview_id(preview.id)
					.principal(self.access.identity.subject.clone())
					.credential_id(self.access.identity.credential_id)
					.expires_at(expires_at)
					.sent_bytes(0)
					.received_bytes(0)
					.requests(0)
					.max_sent_bytes(settings.limits.sent_bytes as i64)
					.max_received_bytes(settings.limits.received_bytes as i64)
					.max_requests(settings.limits.requests as i32)
					.created_at(Utc::now())
					.finish(),
			)
			.await
			.map_err(|error| NativeError::External(error.to_string()))?;
		preview.egress_expires_at = Some(expires_at);
		Ok(())
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

/// Accept the long-lived `serve` operation and stage its inputs in the
/// caller's authority transaction. The Pod starts only after every File version
/// of the pinned revision was written; the wait for readiness happens outside.
pub(crate) async fn dispatch_accept(
	store: &Store,
	access: &mut Access,
	preview: &LivePreview,
	files: Vec<FileEntry>,
	command: &str,
) -> NativeResult<()> {
	use aidash_application::ports::capabilities::runner::RunnerTransport as _;
	let wire = files
		.iter()
		.map(|file| {
			serde_json::json!({"file_id":file.file_id,"path":file.path,"digest":file.digest,"scope":file.scope,"size":file.size})
		})
		.collect::<Vec<_>>();
	let mut input = serde_json::json!({
		"operation_id": preview.id,
		"area_id": preview.area_id,
		"epoch": 1,
		"digest": format!("{:x}", sha2::Sha256::digest(format!("{}\0{}\0{}", preview.area_id, preview.revision, preview.port).as_bytes())),
		"kind": "serve",
		"code": command,
		"seconds": preview.limits.absolute_seconds,
		"files": wire,
		"port": preview.port,
	});
	if let Some(token) = egress_token(store, preview)? {
		let settings = store
			.capabilities
			.0
			.egress
			.as_ref()
			.expect("a token requires egress settings");
		input["egress"] = serde_json::json!({"url": settings.endpoint, "token": token});
	}
	let runner = crate::bootstrap::operation_runner(store)?;
	let observed = runner
		.request("POST", "/v1/operations", Some(&input))
		.await
		.map_err(|error| NativeError::External(error.to_string()))?;
	if observed["status"].as_str() != Some("awaiting_files") {
		return Ok(());
	}
	for file in &files {
		let mut offset = 0_u64;
		loop {
			let bytes = store.capabilities.read_chunk(access, file, offset).await?;
			if bytes.is_empty() && offset < file.size {
				return Err(NativeError::Conflict("OBJECT_INTEGRITY".into()));
			}
			use base64::Engine as _;
			runner
				.request(
					"POST",
					&format!(
						"/v1/operations/{}/inputs/{}?offset={offset}",
						preview.id, file.file_id
					),
					Some(
						&serde_json::json!({"data": base64::engine::general_purpose::STANDARD.encode(&bytes)}),
					),
				)
				.await
				.map_err(|error| NativeError::External(error.to_string()))?;
			offset += bytes.len() as u64;
			if offset >= file.size {
				break;
			}
		}
	}
	runner
		.request(
			"POST",
			&format!("/v1/operations/{}/start", preview.id),
			None,
		)
		.await
		.map_err(|error| NativeError::External(error.to_string()))?;
	Ok(())
}

/// Wait up to 120 seconds for the declared port to accept, reading only the
/// status and the bounded startup log. Never returns a viewing address.
pub(crate) async fn wait_ready(store: &Store, preview: &LivePreview) -> NativeResult<Dispatch> {
	use aidash_application::ports::capabilities::runner::RunnerTransport as _;
	let runner = crate::bootstrap::operation_runner(store)?;
	let deadline =
		tokio::time::Instant::now() + std::time::Duration::from_secs(START_SECONDS as u64);
	loop {
		let observed = runner
			.request("GET", &format!("/v1/operations/{}", preview.id), None)
			.await
			.map_err(|error| NativeError::External(error.to_string()))?;
		let serve = &observed["serve"];
		let log = serve["startup_log"].as_str().unwrap_or_default().to_owned();
		match serve["status"].as_str() {
			Some("running") => {
				let Some(pod_host) = serve["pod_host"].as_str().filter(|host| !host.is_empty())
				else {
					return Ok(Dispatch {
						pod_host: None,
						startup_log: log,
						started_at: Utc::now(),
						failure: Some("the preview Pod reported no address".into()),
					});
				};
				let started_at = observed["started_at"]
					.as_f64()
					.and_then(|seconds| {
						DateTime::from_timestamp_micros((seconds * 1_000_000.0) as i64)
					})
					.unwrap_or_else(Utc::now);
				return Ok(Dispatch {
					pod_host: Some(pod_host.to_owned()),
					startup_log: log,
					started_at,
					failure: None,
				});
			}
			Some("stopped" | "failed")
				if matches!(
					observed["status"].as_str(),
					Some("failed" | "cancelled" | "uncertain")
				) =>
			{
				return Ok(Dispatch {
					pod_host: None,
					startup_log: log,
					started_at: Utc::now(),
					failure: Some(
						serve["reason"]
							.as_str()
							.unwrap_or("the preview never started")
							.to_owned(),
					),
				});
			}
			_ => {}
		}
		if tokio::time::Instant::now() >= deadline {
			return Ok(Dispatch {
				pod_host: None,
				startup_log: log,
				started_at: Utc::now(),
				failure: Some("the preview port did not accept connections in time".into()),
			});
		}
		tokio::time::sleep(std::time::Duration::from_millis(300)).await;
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
	use aidash_application::ports::capabilities::runner::RunnerTransport as _;
	let runner = crate::bootstrap::operation_runner(store)?;
	if runner
		.request(
			"POST",
			&format!("/v1/operations/{}/cancel", preview.id),
			None,
		)
		.await
		.is_err()
	{
		return Ok(false);
	}
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
	loop {
		let observed = runner
			.request("GET", &format!("/v1/operations/{}", preview.id), None)
			.await
			.map_err(|error| NativeError::External(error.to_string()))?;
		if observed["termination_confirmed"] == true {
			return Ok(true);
		}
		if tokio::time::Instant::now() >= deadline {
			return Ok(false);
		}
		tokio::time::sleep(std::time::Duration::from_millis(300)).await;
	}
}

/// Mark every unsettled Live preview of `run` as stopping inside the caller's
/// transaction. A cancellation hook calls this so no Pod outlives the Run.
pub(crate) async fn mark_stopped_for_run(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	run: Uuid,
	reason: StopReason,
) -> NativeResult<()> {
	mark_stopped(tx, reason, Some(run), None).await
}

/// Mark every unsettled Live preview of `tenant` as stopping.
pub(crate) async fn mark_stopped_for_tenant(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	tenant: &str,
	reason: StopReason,
) -> NativeResult<()> {
	mark_stopped(tx, reason, None, Some(tenant)).await
}

async fn mark_stopped(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	reason: StopReason,
	run: Option<Uuid>,
	tenant: Option<&str>,
) -> NativeResult<()> {
	use reinhardt::query::{
		Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	};
	let mut statement = Query::update()
		.table(Alias::new("core_live_previews"))
		.value_expr(Alias::new("state"), Expr::value("stopped"))
		.value_expr(Alias::new("stop_reason"), Expr::value(reason.as_str()))
		.value_expr(Alias::new("termination_confirmed"), Expr::value(false))
		.value_expr(Alias::new("updated_at"), Expr::cust("CURRENT_TIMESTAMP"))
		.and_where(Expr::col(Alias::new("state")).ne("stopped"))
		.to_owned();
	if let Some(run) = run {
		statement = statement
			.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(run)))
			.to_owned();
	}
	if let Some(tenant) = tenant {
		statement = statement
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant.to_owned())))
			.to_owned();
	}
	let (sql, values) = statement.build(PostgresQueryBuilder);
	tx.execute(&sql, reinhardt::db::orm::execution::convert_values(values))
		.await
		.map_err(|error| NativeError::External(error.to_string()))?;
	Ok(())
}

/// The session-lifetime Egress token for a Live preview: `session:<id>.<mac>`.
pub(crate) fn egress_token(store: &Store, preview: &LivePreview) -> NativeResult<Option<String>> {
	let Some(expires_at) = preview.egress_expires_at else {
		return Ok(None);
	};
	Ok(Some(
		crate::apps::execution::repositories::egress::session_token(preview.id, expires_at, store)?,
	))
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
			.map_err(|error| NativeError::External(error.to_string()))?;
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
						.map_err(|error| NativeError::External(error.to_string()))?;
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
				.map_err(|error| NativeError::External(error.to_string()))?;
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
			.map_err(|error| NativeError::External(error.to_string()))?;
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
		.map_err(|error| NativeError::External(error.to_string()))?;
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
	.map_err(|error| NativeError::External(error.to_string()))
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
