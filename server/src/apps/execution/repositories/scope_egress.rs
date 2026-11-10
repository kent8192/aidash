//! Durable scope accounting. Authority comes from the Scratch Space port.
use std::sync::Arc;

use aidash_application::egress::{
	Admission, ConnectionRecord, Direction, EgressScopes, Meter, RequestTimes, ScopeAuthority,
	ScopeSettings,
};
use aidash_application::tenancy::TenancyScope;
use aidash_domain::egress::ScopeId;
use async_trait::async_trait;
use chrono::{NaiveDate, Utc};
use reinhardt::db::orm::Model;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Alias, IntoValue, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use uuid::Uuid;

use crate::apps::execution::capabilities::models::egress::{
	ScopeConnection, ScopeDaily, ScopeEvent, ScopeState,
};
use crate::database::native::Transaction;
use crate::{Error, Result, store::Store};

mod authority;
pub use authority::ScratchAuthority;

/// Durable counters and anomaly state composed with live Scratch Space authority.
#[derive(Clone)]
pub struct DurableScopes {
	store: Store,
	authority: Arc<dyn ScopeAuthority>,
}
impl DurableScopes {
	pub fn new(store: Store, authority: Arc<dyn ScopeAuthority>) -> Self {
		Self { store, authority }
	}

	async fn live(&self, id: &ScopeId) -> aidash_application::Result<ScopeSettings> {
		let settings = self.authority.current(id).await?;
		if &settings.id != id {
			return Err(aidash_application::Error::Forbidden);
		}
		Ok(settings)
	}
	async fn initialize(&self, id: &ScopeId) -> Result<()> {
		let mut tx = self
			.store
			.pool
			.begin(&TenancyScope::tenant(&id.tenant)?)
			.await?;
		let (sql, values) = Query::insert()
			.into_table(Alias::new("egress_scope_state"))
			.columns(
				[
					"scope_id",
					"tenant",
					"node",
					"suspended",
					"window_started_at",
					"connections_in_window",
				]
				.map(Alias::new),
			)
			.values_panic([
				IntoValue::into_value(id.scratch_space),
				IntoValue::into_value(id.tenant.clone()),
				IntoValue::into_value(id.node.clone()),
				IntoValue::into_value(false),
				IntoValue::into_value(Utc::now()),
				IntoValue::into_value(0i32),
			])
			.on_conflict(
				OnConflict::column(Alias::new("scope_id"))
					.do_nothing()
					.to_owned(),
			)
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		let _ = locked_state(&mut tx, id).await?;
		tx.commit().await
	}
	async fn charge(
		&self,
		settings: &ScopeSettings,
		direction: Direction,
		bytes: u64,
		day: NaiveDate,
		service: Option<(&str, aidash_domain::egress::Limits)>,
		forwarded: bool,
	) -> Result<()> {
		let current = self.live(&settings.id).await.map_err(Error::from)?;
		let bytes = i64::try_from(bytes).map_err(|_| Error::Forbidden)?;
		let mut tx = self
			.store
			.pool
			.begin(&TenancyScope::tenant(&current.id.tenant)?)
			.await?;
		let state = locked_state(&mut tx, &current.id).await?;
		if state.suspended {
			return Err(Error::Conflict("egress_suspended".into()));
		}
		let mut total = daily(&mut tx, &current.id, day, "").await?;
		let max = match direction {
			Direction::Sent => current.budget.sent_bytes,
			Direction::Received => current.budget.received_bytes,
		};
		let mut exhausted = charge_row(&mut total, direction, bytes, max, forwarded)?;
		let service_row = if let Some((name, limits)) = service {
			let mut row = daily(&mut tx, &current.id, day, name).await?;
			let max = match direction {
				Direction::Sent => limits.sent_bytes,
				Direction::Received => limits.received_bytes,
			};
			exhausted |= charge_row(&mut row, direction, bytes, max, forwarded)?;
			Some(row)
		} else {
			None
		};
		ScopeDaily::objects()
			.save_with_executor(&mut *tx, &total)
			.await
			.map_err(reinhardt::core::exception::Error::from)?;
		if let Some(row) = service_row {
			ScopeDaily::objects()
				.save_with_executor(&mut *tx, &row)
				.await
				.map_err(reinhardt::core::exception::Error::from)?;
		}
		tx.commit().await?;
		if exhausted {
			return Err(Error::Conflict("egress_budget_exhausted".into()));
		}
		Ok(())
	}
	async fn refund_usage(
		&self,
		id: &ScopeId,
		direction: Direction,
		bytes: u64,
		day: NaiveDate,
		service: Option<&str>,
	) -> Result<()> {
		let bytes = i64::try_from(bytes).map_err(|_| Error::Forbidden)?;
		let mut tx = self
			.store
			.pool
			.begin(&TenancyScope::tenant(&id.tenant)?)
			.await?;
		let _ = locked_state(&mut tx, id).await?;
		for name in [Some(""), service].into_iter().flatten() {
			let mut row = daily(&mut tx, id, day, name).await?;
			let counter = match direction {
				Direction::Sent => &mut row.sent_bytes,
				Direction::Received => &mut row.received_bytes,
			};
			*counter = counter
				.checked_sub(bytes)
				.filter(|v| *v >= 0)
				.ok_or(Error::Forbidden)?;
			ScopeDaily::objects()
				.save_with_executor(&mut *tx, &row)
				.await
				.map_err(reinhardt::core::exception::Error::from)?;
		}
		tx.commit().await
	}
}

async fn locked_state(tx: &mut Transaction, id: &ScopeId) -> Result<ScopeState> {
	ScopeState::objects()
		.filter(ScopeState::field_scope_id().eq(id.scratch_space))
		.filter(ScopeState::field_tenant().eq(id.tenant.clone()))
		.filter(ScopeState::field_node().eq(id.node.clone()))
		.select_for_update()
		.all_with_executor(&mut **tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop()
		.ok_or(Error::Forbidden)
}
async fn daily(
	tx: &mut Transaction,
	id: &ScopeId,
	day: NaiveDate,
	service: &str,
) -> Result<ScopeDaily> {
	if let Some(row) = ScopeDaily::objects()
		.filter(ScopeDaily::field_scope_id().eq(id.scratch_space))
		.filter(ScopeDaily::field_tenant().eq(id.tenant.clone()))
		.filter(ScopeDaily::field_node().eq(id.node.clone()))
		.filter(ScopeDaily::field_day().eq(day.to_string()))
		.filter(ScopeDaily::field_service().eq(service.to_owned()))
		.all_with_executor(&mut **tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop()
	{
		return Ok(row);
	}
	let row = ScopeDaily::build()
		.id(Uuid::new_v4())
		.tenant(id.tenant.clone())
		.node(id.node.clone())
		.scope_id(id.scratch_space)
		.day(day.to_string())
		.service(service.to_owned())
		.sent_bytes(0)
		.received_bytes(0)
		.finish();
	ScopeDaily::objects()
		.insert_with_executor(&mut **tx, &row)
		.await
		.map_err(reinhardt::core::exception::Error::from)?;
	Ok(row)
}
fn add(row: &mut ScopeDaily, direction: Direction, bytes: i64, max: u64) -> Result<()> {
	let max = max.min(i64::MAX as u64);
	let counter = match direction {
		Direction::Sent => &mut row.sent_bytes,
		Direction::Received => &mut row.received_bytes,
	};
	let next = counter
		.checked_add(bytes)
		.filter(|v| *v >= 0 && (*v as u64) <= max)
		.ok_or_else(|| Error::Conflict("egress_budget_exhausted".into()))?;
	*counter = next;
	Ok(())
}
fn charge_row(
	row: &mut ScopeDaily,
	direction: Direction,
	bytes: i64,
	max: u64,
	forwarded: bool,
) -> Result<bool> {
	if !forwarded {
		add(row, direction, bytes, max)?;
		return Ok(false);
	}
	let counter = match direction {
		Direction::Sent => &mut row.sent_bytes,
		Direction::Received => &mut row.received_bytes,
	};
	*counter = counter.saturating_add(bytes);
	Ok(bytes > 0 && (*counter as u64) >= max.min(i64::MAX as u64))
}
// PostgreSQL counters are signed; reaching their maximum must fail closed even
// if a future authority supplies a larger unsigned budget.
fn budget_available(row: &ScopeDaily, sent: u64, received: u64) -> Result<()> {
	if row.sent_bytes as u64 >= sent.min(i64::MAX as u64)
		|| row.received_bytes as u64 >= received.min(i64::MAX as u64)
	{
		return Err(Error::Conflict("egress_budget_exhausted".into()));
	}
	Ok(())
}
async fn check_budget(
	tx: &mut Transaction,
	settings: &ScopeSettings,
	service: Option<(&str, aidash_domain::egress::Limits)>,
) -> Result<()> {
	let day = Utc::now().date_naive();
	let total = daily(tx, &settings.id, day, "").await?;
	budget_available(
		&total,
		settings.budget.sent_bytes,
		settings.budget.received_bytes,
	)?;
	if let Some((name, limits)) = service {
		let row = daily(tx, &settings.id, day, name).await?;
		budget_available(&row, limits.sent_bytes, limits.received_bytes)?;
	}
	Ok(())
}
fn application_error(error: Error) -> aidash_application::Error {
	match error {
		Error::Conflict(code)
			if code == "egress_budget_exhausted" || code == "egress_suspended" =>
		{
			aidash_application::Error::Refused(code)
		}
		other => other.into(),
	}
}

#[async_trait]
impl EgressScopes for DurableScopes {
	async fn resolve(&self, credential: &str) -> aidash_application::Result<ScopeSettings> {
		let scope = self.authority.authenticate(credential).await?;
		self.initialize(&scope.id)
			.await
			.map_err(application_error)?;
		self.live(&scope.id).await
	}
	async fn admit_external(
		&self,
		credential: &str,
		service: &str,
		method: &str,
		path: &str,
		digest: &str,
	) -> aidash_application::Result<Admission> {
		let settings = self.resolve(credential).await?;
		let settings = self.check_active(&settings, credential).await?;
		let mut admission = self
			.authority
			.admit_external(credential, &settings.id, service, method, path, digest)
			.await?;
		if admission.operation.is_some()
			|| admission.session.is_some()
			|| admission.service != service
		{
			return Err(aidash_application::Error::Forbidden);
		}
		let limits = admission.definition.limits;
		let result: Result<()> = async {
			let mut tx = self
				.store
				.pool
				.begin(&TenancyScope::tenant(&settings.id.tenant)?)
				.await?;
			let state = locked_state(&mut tx, &settings.id).await?;
			if state.suspended {
				return Err(Error::Conflict("egress_suspended".into()));
			}
			check_budget(&mut tx, &settings, limits.map(|l| (service, l))).await?;
			tx.commit().await
		}
		.await;
		result.map_err(application_error)?;
		admission.meter = Arc::new(ScopedMeter::new(
			self.clone(),
			settings,
			service,
			limits,
			admission.meter,
		));
		Ok(admission)
	}
	async fn admit_connection(&self, settings: &ScopeSettings) -> aidash_application::Result<()> {
		let current = self.live(&settings.id).await?;
		let result: Result<bool> = async {
			let mut tx = self
				.store
				.pool
				.begin(&TenancyScope::tenant(&current.id.tenant)?)
				.await?;
			let mut state = locked_state(&mut tx, &current.id).await?;
			if state.suspended {
				return Err(Error::Conflict("egress_suspended".into()));
			}
			check_budget(&mut tx, &current, None).await?;
			let now = Utc::now();
			if now.signed_duration_since(state.window_started_at) >= chrono::Duration::minutes(1) {
				state.window_started_at = now;
				state.connections_in_window = 0;
			}
			state.connections_in_window = state.connections_in_window.saturating_add(1);
			let suspended = u32::try_from(state.connections_in_window).unwrap_or(u32::MAX)
				> current.anomaly.connections_per_minute;
			if suspended {
				state.suspended = true;
				let event = ScopeEvent::build()
					.id(Uuid::new_v4())
					.tenant(current.id.tenant.clone())
					.node(current.id.node.clone())
					.scope_id(current.id.scratch_space)
					.kind("suspended".to_owned())
					.created_at(now)
					.delivered_at(None)
					.finish();
				ScopeEvent::objects()
					.insert_with_executor(&mut *tx, &event)
					.await
					.map_err(reinhardt::core::exception::Error::from)?;
			}
			ScopeState::objects()
				.save_with_executor(&mut *tx, &state)
				.await
				.map_err(reinhardt::core::exception::Error::from)?;
			tx.commit().await?;
			Ok(suspended)
		}
		.await;
		match result {
			Ok(true) => {
				tracing::warn!(tenant = %current.id.tenant, node = %current.id.node, scope = %current.id.scratch_space, "egress scope suspended for anomalous traffic");
				Err(aidash_application::Error::Refused(
					"egress_suspended".into(),
				))
			}
			Ok(false) => Ok(()),
			Err(error) => Err(application_error(error)),
		}
	}
	async fn check_active(
		&self,
		settings: &ScopeSettings,
		credential: &str,
	) -> aidash_application::Result<ScopeSettings> {
		let authenticated = self.authority.authenticate(credential).await?;
		if authenticated.id != settings.id {
			return Err(aidash_application::Error::Forbidden);
		}
		let current = self.live(&settings.id).await?;
		let result: Result<()> = async {
			let mut tx = self
				.store
				.pool
				.begin(&TenancyScope::tenant(&current.id.tenant)?)
				.await?;
			let state = locked_state(&mut tx, &current.id).await?;
			if state.suspended {
				return Err(Error::Conflict("egress_suspended".into()));
			}
			tx.commit().await
		}
		.await;
		result.map_err(application_error)?;
		Ok(current)
	}
	async fn debit(
		&self,
		settings: &ScopeSettings,
		direction: Direction,
		bytes: u64,
		day: NaiveDate,
	) -> aidash_application::Result<()> {
		self.charge(settings, direction, bytes, day, None, false)
			.await
			.map_err(application_error)
	}
	async fn refund(
		&self,
		settings: &ScopeSettings,
		direction: Direction,
		bytes: u64,
		day: NaiveDate,
	) -> aidash_application::Result<()> {
		self.refund_usage(&settings.id, direction, bytes, day, None)
			.await
			.map_err(application_error)
	}

	async fn record(&self, record: ConnectionRecord) -> aidash_application::Result<()> {
		let result: Result<()> = async {
			let mut tx = self
				.store
				.pool
				.begin(&TenancyScope::tenant(&record.scope.tenant)?)
				.await?;
			let existing = ScopeConnection::objects()
				.filter(ScopeConnection::field_id().eq(record.id))
				.all_with_executor(&mut *tx)
				.await
				.map_err(reinhardt::core::exception::Error::from)?
				.pop();
			let row = ScopeConnection::build()
				.id(record.id)
				.tenant(record.scope.tenant)
				.node(record.scope.node)
				.scope_id(record.scope.scratch_space)
				.host(record.host)
				.port(i32::from(record.port))
				.sent_bytes(i64::try_from(record.sent_bytes).map_err(|_| Error::Forbidden)?)
				.received_bytes(i64::try_from(record.received_bytes).map_err(|_| Error::Forbidden)?)
				.duration_ms(i64::try_from(record.duration_ms).map_err(|_| Error::Forbidden)?)
				.outcome(record.outcome)
				.error(record.error)
				.wait_ms(
					record
						.wait_ms
						.map(i64::try_from)
						.transpose()
						.map_err(|_| Error::Forbidden)?,
				)
				.created_at(
					existing
						.as_ref()
						.map_or_else(Utc::now, |row| row.created_at),
				)
				.finish();
			if existing.is_some() {
				ScopeConnection::objects()
					.save_with_executor(&mut *tx, &row)
					.await
					.map_err(reinhardt::core::exception::Error::from)?;
			} else {
				ScopeConnection::objects()
					.insert_with_executor(&mut *tx, &row)
					.await
					.map_err(reinhardt::core::exception::Error::from)?;
			}
			tx.commit().await
		}
		.await;
		result.map_err(application_error)
	}
	async fn lift_suspension(&self, id: &ScopeId) -> aidash_application::Result<()> {
		let result: Result<()> = async {
			let mut tx = self
				.store
				.pool
				.begin(&TenancyScope::tenant(&id.tenant)?)
				.await?;
			let mut state = locked_state(&mut tx, id).await?;
			state.suspended = false;
			state.connections_in_window = 0;
			state.window_started_at = Utc::now();
			ScopeState::objects()
				.save_with_executor(&mut *tx, &state)
				.await
				.map_err(reinhardt::core::exception::Error::from)?;
			tx.commit().await
		}
		.await;
		result.map_err(application_error)
	}
}
/// A decision about the request, as opposed to a store that could not be asked.
/// The gateway charges the whole amount again after anything else (see
/// [`Meter::charge`]), so only a decision may leave bytes committed.
fn decides(error: &aidash_application::Error) -> bool {
	use aidash_application::Error;
	match error {
		Error::Forbidden | Error::Unauthorized | Error::AuthorityRefused(_) => true,
		Error::Refused(code) => code == "egress_budget_exhausted" || code == "egress_suspended",
		Error::Conflict(code) => {
			code == aidash_domain::egress::LIMIT_EXCEEDED
				|| code == aidash_domain::egress::OPERATION_EXPIRED
				|| code == aidash_domain::egress::OPERATION_INACTIVE
		}
		_ => false,
	}
}

/// What one side of a debit has taken ahead of a charge both sides have
/// acknowledged.
#[derive(Default)]
struct Taken {
	/// Bytes this side committed on an earlier attempt of the charge.
	bytes: u64,
	/// The decision it answered them with, which a later attempt still owes the
	/// caller.
	stop: Option<aidash_application::Error>,
}

/// Both sides of one direction's debit.
#[derive(Default)]
struct Ahead {
	scope: Taken,
	audit: Taken,
}

/// Charges the daily scope budget, then the request's own meter. The gateway
/// charges the whole amount again after any error that is not a decision, so
/// each side remembers what it has already committed and takes only the rest.
struct ScopedMeter {
	durable: DurableScopes,
	settings: ScopeSettings,
	service: String,
	limits: Option<aidash_domain::egress::Limits>,
	inner: Arc<dyn Meter>,
	sent: tokio::sync::Mutex<Ahead>,
	received: tokio::sync::Mutex<Ahead>,
}
impl ScopedMeter {
	fn new(
		durable: DurableScopes,
		settings: ScopeSettings,
		service: &str,
		limits: Option<aidash_domain::egress::Limits>,
		inner: Arc<dyn Meter>,
	) -> Self {
		Self {
			durable,
			settings,
			service: service.to_owned(),
			limits,
			inner,
			sent: Default::default(),
			received: Default::default(),
		}
	}
}
#[async_trait]
impl Meter for ScopedMeter {
	async fn check_active(&self) -> aidash_application::Result<()> {
		self.inner.check_active().await
	}
	async fn charge(&self, direction: Direction, bytes: usize) -> aidash_application::Result<()> {
		let bytes = bytes as u64;
		// Held to the end, so that charges in one direction never read each
		// other's partial progress. A caller that gives up drops it.
		let mut ahead = match direction {
			Direction::Sent => &self.sent,
			Direction::Received => &self.received,
		}
		.lock()
		.await;
		// What a side still owes. An empty charge still asks both sides: it is
		// how a stream rechecks.
		let still_owed = |taken: &Taken| {
			if bytes == 0 || taken.bytes < bytes {
				Some(bytes - taken.bytes.min(bytes))
			} else {
				None
			}
		};
		let mut unavailable = None;
		// Each side's progress is recorded as soon as it answers, before the
		// other is asked, so that a caller whose timeout cancels this call
		// between the two still charges each side once.
		if let Some(owed) = still_owed(&ahead.scope) {
			let result = self
				.durable
				.charge(
					&self.settings,
					direction,
					owed,
					Utc::now().date_naive(),
					self.limits.map(|l| (self.service.as_str(), l)),
					true,
				)
				.await
				.map_err(application_error);
			match result {
				Ok(()) => ahead.scope.bytes += owed,
				Err(error) if decides(&error) => {
					ahead.scope.bytes += owed;
					ahead.scope.stop = Some(error);
				}
				Err(error) => unavailable = Some(error),
			}
		}
		if let Some(owed) = still_owed(&ahead.audit) {
			match self.inner.charge(direction, owed as usize).await {
				Ok(()) => ahead.audit.bytes += owed,
				Err(error) if decides(&error) => {
					ahead.audit.bytes += owed;
					ahead.audit.stop = Some(error);
				}
				Err(error) => unavailable = unavailable.or(Some(error)),
			}
		}
		if let Some(error) = unavailable {
			return Err(error);
		}
		// Both sides have answered, so the caller's charge is acknowledged.
		ahead.scope.bytes = ahead.scope.bytes.saturating_sub(bytes);
		ahead.audit.bytes = ahead.audit.bytes.saturating_sub(bytes);
		match ahead.scope.stop.take().or(ahead.audit.stop.take()) {
			Some(stop) => Err(stop),
			None => Ok(()),
		}
	}
	async fn finish(
		&self,
		status: u16,
		times: RequestTimes,
		error: Option<&str>,
	) -> aidash_application::Result<()> {
		self.inner.finish(status, times, error).await
	}
}

#[cfg(test)]
mod tests;
