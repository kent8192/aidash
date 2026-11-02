//! Durable scope accounting. Authority comes from the Scratch Space port.
use std::sync::Arc;

use aidash_application::egress::{
	Admission, ConnectionRecord, Direction, EgressScopes, Meter, ScopeAuthority, ScopeSettings,
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
		add(&mut total, direction, bytes, max)?;
		let service_row = if let Some((name, limits)) = service {
			let mut row = daily(&mut tx, &current.id, day, name).await?;
			let max = match direction {
				Direction::Sent => limits.sent_bytes,
				Direction::Received => limits.received_bytes,
			};
			add(&mut row, direction, bytes, max)?;
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
		tx.commit().await
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
		self.check_active(&settings).await?;
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
		admission.meter = Arc::new(ScopedMeter {
			durable: self.clone(),
			settings,
			service: service.to_owned(),
			limits,
			inner: admission.meter,
		});
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
	async fn check_active(&self, settings: &ScopeSettings) -> aidash_application::Result<()> {
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
		result.map_err(application_error)
	}
	async fn debit(
		&self,
		settings: &ScopeSettings,
		direction: Direction,
		bytes: u64,
		day: NaiveDate,
	) -> aidash_application::Result<()> {
		self.charge(settings, direction, bytes, day, None)
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
			let row = ScopeConnection::build()
				.id(Uuid::new_v4())
				.tenant(record.scope.tenant)
				.node(record.scope.node)
				.scope_id(record.scope.scratch_space)
				.host(record.host)
				.port(i32::from(record.port))
				.sent_bytes(i64::try_from(record.sent_bytes).map_err(|_| Error::Forbidden)?)
				.received_bytes(i64::try_from(record.received_bytes).map_err(|_| Error::Forbidden)?)
				.duration_ms(i64::try_from(record.duration_ms).map_err(|_| Error::Forbidden)?)
				.outcome(record.outcome)
				.created_at(Utc::now())
				.finish();
			ScopeConnection::objects()
				.insert_with_executor(&mut *tx, &row)
				.await
				.map_err(reinhardt::core::exception::Error::from)?;
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
struct ScopedMeter {
	durable: DurableScopes,
	settings: ScopeSettings,
	service: String,
	limits: Option<aidash_domain::egress::Limits>,
	inner: Arc<dyn Meter>,
}
#[async_trait]
impl Meter for ScopedMeter {
	async fn charge(&self, direction: Direction, bytes: usize) -> aidash_application::Result<()> {
		let day = Utc::now().date_naive();
		self.durable
			.charge(
				&self.settings,
				direction,
				bytes as u64,
				day,
				self.limits.map(|l| (self.service.as_str(), l)),
			)
			.await
			.map_err(application_error)?;
		if let Err(error) = self.inner.charge(direction, bytes).await {
			if self
				.durable
				.refund_usage(
					&self.settings.id,
					direction,
					bytes as u64,
					day,
					self.limits.map(|_| self.service.as_str()),
				)
				.await
				.is_err()
			{
				tracing::error!("Scope External service reservation refund failed");
			}
			return Err(error);
		}
		Ok(())
	}
	async fn finish(
		&self,
		status: u16,
		elapsed_ms: u64,
		error: Option<&str>,
	) -> aidash_application::Result<()> {
		self.inner.finish(status, elapsed_ms, error).await
	}
}

#[cfg(test)]
mod tests;
