//! Authority bindings, pending attempts and trust reads keep their existing pools and lock boundaries.
use crate::apps::federation::transactions::serializers::authority::{Origin, Preflight};
use crate::{
	Error, Result,
	authorization::{access::Access, identity::SubjectIdentity},
	federation::Federation,
};
use reinhardt::query::{
	Alias, ColumnRef, Expr, IntoValue, OnConflict, PostgresQueryBuilder, Query, SimpleExpr,
};
use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;
impl From<&SubjectIdentity> for Origin {
	fn from(identity: &SubjectIdentity) -> Self {
		Self {
			credential_id: identity.credential_id,
			tenant: identity.tenant.clone(),
			subject: identity.subject.clone(),
		}
	}
}
impl Origin {
	fn identity(&self) -> SubjectIdentity {
		SubjectIdentity {
			http_session: None,
			credential_id: self.credential_id,
			tenant: self.tenant.clone(),
			subject: self.subject.clone(),
		}
	}
}

pub(crate) async fn control(tx: &mut Transaction<'_, Postgres>) -> Result<()> {
	sqlx::query(
		&Query::select()
			.expr(Expr::cust(
				"set_config('aidash.transaction_control','authority',true)",
			))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **tx)
	.await?;
	Ok(())
}

pub(crate) fn control_federation(f: &Federation) -> Federation {
	let mut f = f.clone();
	f.store.pool = f.store.control_pool.clone();
	f
}

pub(crate) async fn access(f: &Federation, origin: &Origin) -> Result<Access> {
	let mut access = Access::begin(&control_federation(f).store, &origin.identity()).await?;
	control(&mut access.tx).await?;
	Ok(access)
}

pub(crate) async fn bind(
	tx: &mut Transaction<'_, Postgres>,
	table: &str,
	id: Uuid,
	binding: &Value,
) -> Result<()> {
	{
		let query_bind_1 = id;
		let query_bind_2 = binding;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new(table))
				.columns([Alias::new("id"), Alias::new("binding")])
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.to_owned(),
				)
				.on_conflict(OnConflict::columns(["id"]).do_nothing().to_owned())
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await?
	};
	let stored: Value = {
		let query_bind_1 = id;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("binding"))
				.from(Alias::new(table))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&mut **tx)
		.await?
	};
	if stored != *binding {
		return Err(Error::Conflict(
			"transaction authority binding is immutable".into(),
		));
	}
	Ok(())
}

pub(crate) async fn binding<T: serde::de::DeserializeOwned>(
	f: &Federation,
	table: &str,
	id: Uuid,
) -> Result<Option<T>> {
	binding_with(&f.store.control_pool, table, id).await
}

pub(crate) async fn binding_with<'e, T: serde::de::DeserializeOwned>(
	executor: impl sqlx::Executor<'e, Database = Postgres>,
	table: &str,
	id: Uuid,
) -> Result<Option<T>> {
	let value: Option<Value> = {
		let query_bind_1 = id;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("binding"))
				.from(Alias::new(table))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(executor)
		.await?
	};
	value
		.map(serde_json::from_value)
		.transpose()
		.map_err(Into::into)
}

pub(crate) async fn match_origin_with<'e>(
	executor: impl sqlx::Executor<'e, Database = Postgres>,
	id: Uuid,
	origin: Option<&Origin>,
) -> Result<()> {
	let stored: Option<Origin> = binding_with(executor, "atomic_subjects", id).await?;
	if stored.as_ref() != origin {
		return Err(Error::Forbidden);
	}
	Ok(())
}

pub(crate) async fn run(access: &mut Access, id: Uuid) -> Result<crate::domain::Run> {
	{
		let query_bind_1 = id;
		aidash_server::database::query_as(
			&Query::select()
				.column(ColumnRef::Asterisk)
				.from(Alias::new("runs"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **access.tx)
		.await?
	}
	.ok_or(Error::Forbidden)
}

pub(crate) async fn mapped(f: &Federation, input: &Preflight) -> Result<Access> {
	if input.coordinator == f.config.node_id {
		return access(f, &input.origin).await;
	}
	let mut access = crate::authorization::peer::access(
		&control_federation(f),
		&input.coordinator,
		&input.origin.tenant,
		&input.origin.subject,
	)
	.await?;
	control(&mut access.tx).await?;
	Ok(access)
}

pub(crate) async fn settle(f: &Federation, id: Uuid, node: &str, outcome: &str) -> Result<()> {
	{
		let query_bind_1 = id;
		let query_bind_2 = node;
		let query_bind_3 = outcome;
		sqlx::query(
			&Query::update()
				.table(Alias::new("atomic_authority_attempts"))
				.value_expr(
					Alias::new("outcome"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_3.to_owned()).into()],
					),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(transaction_id=? AND node_id=? AND outcome IS NULL)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&f.store.control_pool)
		.await?
	};
	Ok(())
}

/// Conservative unknown outcomes are safe to display as pending even before
/// revocation. A terminal acknowledgment is the only completion evidence.
pub(crate) async fn pending(
	f: &Federation,
	tenant: &str,
	credential: Option<Uuid>,
) -> Result<Vec<Uuid>> {
	let mut query = Query::select();
	query
		.distinct()
		.column((Alias::new("a"), Alias::new("transaction_id")))
		.from_as(Alias::new("atomic_authority_attempts"), Alias::new("a"))
		.join(
			reinhardt::query::JoinType::InnerJoin,
			reinhardt::query::TableRef::table_alias(Alias::new("atomic_subjects"), Alias::new("s")),
			Expr::cust("s.id=a.transaction_id"),
		)
		.and_where(Expr::cust("a.outcome IS NULL AND s.binding->>'tenant'=$1"));
	if credential.is_some() {
		query.and_where(Expr::cust("s.binding->>'credential_id'=$2"));
	}
	let sql = query.to_string(PostgresQueryBuilder);
	let mut query = sqlx::query_scalar(&sql).bind(tenant);
	if let Some(credential) = credential {
		query = query.bind(credential.to_string());
	}
	Ok(query.fetch_all(&f.store.control_pool).await?)
}

pub(crate) async fn scoped(f: &Federation, id: Uuid) -> Result<bool> {
	Ok(binding::<Origin>(f, "atomic_subjects", id).await?.is_some())
}

pub(crate) async fn trusted(access: &mut Access, node: &str) -> Result<()> {
	let trusted: Option<bool> = {
		let query_bind_1 = node;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("enabled"))
				.from(Alias::new("atomic_peer_trust"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(node_id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.lock(reinhardt::query::LockType::Share)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **access.tx)
		.await?
	};
	if trusted != Some(true) {
		return Err(Error::Forbidden);
	}
	Ok(())
}

pub(crate) async fn pending_peer(f: &Federation, node: &str) -> Result<Vec<Uuid>> {
	Ok({
		let query_bind_1 = node;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("transaction_id"))
				.from(Alias::new("atomic_authority_attempts"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(node_id=? AND outcome IS NULL)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(&f.store.control_pool)
		.await?
	})
}

pub(crate) async fn bind_native(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	table: &str,
	id: Uuid,
	binding: &Value,
) -> Result<()> {
	use reinhardt::db::orm::execution::convert_values;
	let (sql, values) = Query::insert()
		.into_table(Alias::new(table))
		.columns([Alias::new("id"), Alias::new("binding")])
		.values_panic([
			IntoValue::into_value(id),
			IntoValue::into_value(binding.clone()),
		])
		.on_conflict(OnConflict::columns(["id"]).do_nothing().to_owned())
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	if native_binding(tx, table, id).await?.as_ref() != Some(binding) {
		return Err(Error::Conflict(
			"transaction authority binding is immutable".into(),
		));
	}
	Ok(())
}

pub(crate) async fn native_binding(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	table: &str,
	id: Uuid,
) -> Result<Option<Value>> {
	use reinhardt::db::orm::execution::convert_values;
	let (sql, values) = Query::select()
		.column(Alias::new("binding"))
		.from(Alias::new(table))
		.and_where(Expr::col("id").eq(Expr::value(id)))
		.build(PostgresQueryBuilder);
	tx.fetch_optional(&sql, convert_values(values))
		.await?
		.map(|row| match row.data.get("binding") {
			Some(reinhardt::db::backends::QueryValue::Json(Some(value))) => Ok((**value).clone()),
			_ => Err(Error::External("invalid authority binding".into())),
		})
		.transpose()
}

pub(crate) async fn match_origin_native(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	id: Uuid,
	origin: Option<&Origin>,
) -> Result<()> {
	let stored: Option<Origin> = native_binding(tx, "atomic_subjects", id)
		.await?
		.map(serde_json::from_value)
		.transpose()?;
	if stored.as_ref() != origin {
		return Err(Error::Forbidden);
	}
	Ok(())
}

pub(crate) async fn insert_attempt(access: &mut Access, id: Uuid, node: &str) -> Result<()> {
	{
		let query_bind_1 = id;
		let query_bind_2 = node;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("atomic_authority_attempts"))
				.columns([Alias::new("transaction_id"), Alias::new("node_id")])
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.to_owned(),
				)
				.on_conflict(
					OnConflict::columns(["transaction_id", "node_id"])
						.do_nothing()
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **access.tx)
		.await?
	};
	Ok(())
}

pub(crate) async fn attempt(access: &mut Access, id: Uuid, node: &str) -> Result<Option<String>> {
	Ok({
		let query_bind_1 = id;
		let query_bind_2 = node;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("node_id"))
				.from(Alias::new("atomic_authority_attempts"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(transaction_id=? AND node_id=? AND outcome IS NULL)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **access.tx)
		.await?
	})
}

pub(crate) async fn status_with<'e>(
	executor: impl sqlx::Executor<'e, Database = sqlx::Postgres>,
	id: Uuid,
) -> Result<crate::apps::federation::transactions::serializers::contracts::Status> {
	{
		let query_bind_1 = id;
		sqlx::query_as(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
				))
				.from(reinhardt::query::Alias::new("atomic_coordinators"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_optional(executor)
		.await?
	}
	.ok_or_else(|| Error::NotFound("transaction".into()))
}
