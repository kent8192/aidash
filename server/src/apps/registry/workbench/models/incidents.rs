//! Incident records, retention, and source-attributed history.
use super::{AgentIncident, AgentIncidentEvent};
use crate::apps::registry::workbench::serializers::incident::{Incident, IncidentEvent};
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, QueryRow};
use reinhardt::query::{
	Alias, ColumnRef, Condition, Expr, ExprTrait, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use serde_json::Value;
use uuid::Uuid;

impl AgentIncident {
	pub(crate) async fn read(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		lock: bool,
	) -> Result<Incident> {
		let query = Self::objects().filter(Self::field_id().eq(id));
		let rows = if lock {
			query.select_for_update().all_with_executor(tx).await
		} else {
			query.all_with_executor(tx).await
		}
		.map_err(FrameworkError::from)?;
		rows.into_iter()
			.next()
			.map(Into::into)
			.ok_or_else(|| Error::NotFound("incident".into()))
	}

	pub(crate) async fn page(
		tx: &mut dyn TransactionExecutor,
		agent: &str,
		version: &str,
		tenant: Option<&str>,
		cursor: Option<(DateTime<Utc>, Uuid)>,
	) -> Result<Vec<Incident>> {
		let mut query = Query::select();
		query
			.column(ColumnRef::Asterisk)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("agent_id").eq(Expr::value(agent)))
			.and_where(Expr::col("version").eq(Expr::value(version)))
			.order_by(Alias::new("created_at"), Order::Desc)
			.order_by(Alias::new("id"), Order::Desc)
			.limit(100);
		if let Some(tenant) = tenant {
			query.and_where(Expr::col("tenant").eq(Expr::value(tenant)));
		}
		if let Some((at, id)) = cursor {
			query.and_where(
				Condition::any()
					.add(Expr::col("created_at").lt(Expr::value(at)))
					.add(
						Condition::all()
							.add(Expr::col("created_at").eq(Expr::value(at)))
							.add(Expr::col("id").lt(Expr::value(id))),
					),
			);
		}
		let (sql, values) = query.build(PostgresQueryBuilder);
		tx.fetch_all(&sql, convert_values(values))
			.await?
			.into_iter()
			.map(|row| {
				serde_json::from_value(QueryRow::from_backend_row(row).data).map_err(Into::into)
			})
			.collect()
	}

	pub(crate) async fn insert(tx: &mut dyn TransactionExecutor, record: &Incident) -> Result<()> {
		let (sql, values) = Query::insert()
			.into_table(Alias::new(Self::table_name()))
			.columns(
				[
					"id", "tenant", "agent_id", "version", "severity", "owner", "notes", "evidence",
				]
				.map(Alias::new),
			)
			.values_panic([
				IntoValue::into_value(record.id),
				IntoValue::into_value(&record.tenant),
				IntoValue::into_value(&record.agent_id),
				IntoValue::into_value(&record.version),
				IntoValue::into_value(&record.severity),
				IntoValue::into_value(&record.owner),
				IntoValue::into_value(&record.notes),
				IntoValue::into_value(record.evidence.clone()),
			])
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}

	pub(crate) async fn save(tx: &mut dyn TransactionExecutor, record: &Incident) -> Result<()> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(
				Alias::new("revision"),
				Expr::col("revision").add(Expr::value(1_i64)),
			)
			.value_expr(Alias::new("severity"), Expr::value(&record.severity))
			.value_expr(Alias::new("status"), Expr::value(&record.status))
			.value_expr(Alias::new("archived"), Expr::value(record.archived))
			.value_expr(Alias::new("owner"), Expr::value(&record.owner))
			.value_expr(Alias::new("notes"), Expr::value(&record.notes))
			.value_expr(Alias::new("evidence"), Expr::value(record.evidence.clone()))
			.value_expr(Alias::new("resolved_at"), Expr::value(record.resolved_at))
			.value_expr(
				Alias::new("evidence_expires_at"),
				Expr::value(record.evidence_expires_at),
			)
			.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
			.and_where(Expr::col("id").eq(Expr::value(record.id)))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}

	pub(crate) async fn expired(tx: &mut dyn TransactionExecutor) -> Result<Vec<Incident>> {
		Ok(Self::objects()
			.filter(Self::field_status().eq("resolved"))
			.filter(Self::field_evidence_expires_at().lte(Some(Utc::now())))
			.filter(Self::field_evidence_expired_at().is_null())
			.order_by(&["id"])
			.limit(100)
			.select_for_update()
			.skip_locked()
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.into_iter()
			.map(Into::into)
			.collect())
	}

	pub(crate) async fn discard_evidence(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		evidence: Value,
	) -> Result<()> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("evidence"), Expr::value(evidence))
			.value_expr(Alias::new("evidence_expired_at"), Expr::current_timestamp())
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}
}

impl AgentIncidentEvent {
	pub(crate) async fn record(
		tx: &mut dyn TransactionExecutor,
		incident: Uuid,
		actor: &str,
		change: Value,
	) -> Result<()> {
		let (sql, values) = Query::insert()
			.into_table(Alias::new(Self::table_name()))
			.columns(["incident_id", "actor", "change"].map(Alias::new))
			.values_panic([
				IntoValue::into_value(incident),
				IntoValue::into_value(actor),
				IntoValue::into_value(change),
			])
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}

	pub(crate) async fn page(
		tx: &mut dyn TransactionExecutor,
		incident: Uuid,
		limit: usize,
	) -> Result<Vec<IncidentEvent>> {
		Ok(Self::objects()
			.filter(Self::field_incident_id().eq(incident))
			.order_by(&["-id"])
			.limit(limit)
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.into_iter()
			.map(Into::into)
			.collect())
	}
}

use reinhardt::query::IntoValue;
