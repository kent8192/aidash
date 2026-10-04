//! Native draft reads with stable paging and current sharing provenance.
use super::{AgentDraft, AgentDraftShare};
use crate::apps::registry::workbench::serializers::contracts::Draft;
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, QueryRow};
use reinhardt::query::OnConflict;
use reinhardt::query::{
	Alias, ColumnRef, Condition, Expr, ExprTrait, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use serde_json::Value;
use uuid::Uuid;

impl AgentDraft {
	pub(crate) async fn insert(
		tx: &mut dyn TransactionExecutor,
		draft: &Draft,
		managed_id: &str,
	) -> Result<Draft> {
		let (sql, values) = Query::insert()
			.into_table(Alias::new(Self::table_name()))
			.columns(
				[
					"id",
					"tenant",
					"owner",
					"managed_id",
					"entry",
					"documents",
					"release_notes",
					"source_id",
					"source_version",
				]
				.map(Alias::new),
			)
			.values_panic([
				IntoValue::into_value(draft.id),
				IntoValue::into_value(&draft.tenant),
				IntoValue::into_value(&draft.owner),
				IntoValue::into_value(managed_id),
				IntoValue::into_value(draft.entry.clone()),
				IntoValue::into_value(draft.documents.clone()),
				IntoValue::into_value(&draft.release_notes),
				IntoValue::into_value(draft.source_id.clone()),
				IntoValue::into_value(draft.source_version.clone()),
			])
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Self::read(tx, draft.id, false).await
	}

	pub(crate) async fn managed(
		tx: &mut dyn TransactionExecutor,
		managed_id: &str,
	) -> Result<bool> {
		Ok(!Self::objects()
			.filter(Self::field_managed_id().eq(managed_id))
			.limit(1)
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.is_empty())
	}

	pub(crate) async fn save_content(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		entry: Value,
		documents: Value,
		notes: &str,
	) -> Result<Draft> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(
				Alias::new("revision"),
				Expr::col("revision").add(Expr::value(1_i64)),
			)
			.value_expr(Alias::new("entry"), Expr::value(entry))
			.value_expr(Alias::new("documents"), Expr::value(documents))
			.value_expr(Alias::new("release_notes"), Expr::value(notes))
			.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Self::read(tx, id, false).await
	}

	pub(crate) async fn transfer(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		owner: &str,
	) -> Result<()> {
		AgentDraftShare::remove(tx, id, owner).await?;
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("owner"), Expr::value(owner))
			.value_expr(
				Alias::new("revision"),
				Expr::col("revision").add(Expr::value(1_i64)),
			)
			.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}

	pub(crate) async fn archive(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		archived: bool,
	) -> Result<()> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("archived"), Expr::value(archived))
			.value_expr(
				Alias::new("revision"),
				Expr::col("revision").add(Expr::value(1_i64)),
			)
			.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}

	pub(crate) async fn page(
		tx: &mut dyn TransactionExecutor,
		tenant: Option<&str>,
		cursor: Option<(DateTime<Utc>, Uuid)>,
	) -> Result<Vec<Draft>> {
		let mut query = Query::select();
		query
			.column(ColumnRef::Asterisk)
			.from(Alias::new(Self::table_name()))
			.order_by(Alias::new("updated_at"), Order::Desc)
			.order_by(Alias::new("id"), Order::Desc)
			.limit(100);
		if let Some(tenant) = tenant {
			query.and_where(Expr::col("tenant").eq(Expr::value(tenant)));
		}
		if let Some((at, id)) = cursor {
			query.and_where(
				Condition::any()
					.add(Expr::col("updated_at").lt(Expr::value(at)))
					.add(
						Condition::all()
							.add(Expr::col("updated_at").eq(Expr::value(at)))
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

	pub(crate) async fn read(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		lock: bool,
	) -> Result<Draft> {
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
			.ok_or_else(|| Error::NotFound("agent draft".into()))
	}
}

impl AgentDraftShare {
	pub(crate) async fn remove(
		tx: &mut dyn TransactionExecutor,
		draft: Uuid,
		subject: &str,
	) -> Result<()> {
		let (sql, values) = Query::delete()
			.from_table(Alias::new(Self::table_name()))
			.and_where(Expr::col("draft_id").eq(Expr::value(draft)))
			.and_where(Expr::col("subject").eq(Expr::value(subject)))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}

	pub(crate) async fn save(
		tx: &mut dyn TransactionExecutor,
		draft: Uuid,
		subject: &str,
		can_edit: bool,
		digest: &str,
	) -> Result<()> {
		let (sql, values) = Query::insert()
			.into_table(Alias::new(Self::table_name()))
			.columns(["draft_id", "subject", "can_edit", "documents_digest"].map(Alias::new))
			.values_panic([
				IntoValue::into_value(draft),
				IntoValue::into_value(subject),
				IntoValue::into_value(can_edit),
				IntoValue::into_value(digest),
			])
			.on_conflict(
				OnConflict::columns([Alias::new("draft_id"), Alias::new("subject")])
					.update_columns([Alias::new("can_edit"), Alias::new("documents_digest")])
					.to_owned(),
			)
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}

	pub(crate) async fn page(tx: &mut dyn TransactionExecutor, draft: Uuid) -> Result<Vec<Self>> {
		Ok(Self::objects()
			.filter(Self::field_draft_id().eq(draft))
			.order_by(&["subject"])
			.limit(100)
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?)
	}

	pub(crate) async fn current(
		tx: &mut dyn TransactionExecutor,
		draft: Uuid,
		subject: &str,
	) -> Result<Option<Self>> {
		Ok(Self::objects()
			.filter(Self::field_draft_id().eq(draft))
			.filter(Self::field_subject().eq(subject))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.into_iter()
			.next())
	}
}

use reinhardt::query::IntoValue;
