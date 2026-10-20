//! Participant state, visibility barriers, and rollback-only mutation validation.
use super::{AtomicHistory, AtomicParticipant};
use crate::apps::federation::transactions::serializers::contracts::Manifest;
use crate::apps::federation::transactions::services::states::AtomicParticipantPhase;
use crate::database::native::Pool;
use crate::{Error, Result};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, OrmExecutor};
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use serde_json::json;
use uuid::Uuid;

impl AtomicParticipant {
	/// Participant records are cross-node transaction plumbing, which is
	/// Deployment-wide. The same transaction applies the manifest's mutations,
	/// which reach Tenant-owned rows such as `authorization_execution`.
	pub(crate) async fn begin(db: &Pool) -> Result<Box<dyn TransactionExecutor>> {
		db.begin_serializable(&TenancyScope::Deployment(
			DeploymentScopeReason::AtomicParticipant,
		))
		.await?
		.into_executor()
	}

	pub(crate) async fn lock(tx: &mut dyn TransactionExecutor, id: Uuid) -> Result<Option<Self>> {
		Ok(Self::objects()
			.filter(Self::field_id().eq(id))
			.select_for_update()
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop())
	}

	pub(crate) async fn insert(
		tx: &mut dyn TransactionExecutor,
		manifest: &Manifest,
		phase: AtomicParticipantPhase,
	) -> Result<Self> {
		let row = Self::build()
			.id(manifest.id)
			.coordinator(&manifest.coordinator)
			.digest(manifest.digest()?)
			.manifest(json!(manifest).into())
			.phase(phase)
			.finish();
		Ok(Self::objects()
			.insert_with_executor(tx, &row)
			.await
			.map_err(FrameworkError::from)?)
	}

	pub(crate) async fn transition(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		next: AtomicParticipantPhase,
	) -> Result<Self> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("phase"), Expr::value(next.as_str()))
			.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		AtomicHistory::append(
			tx,
			id,
			"participant",
			next.as_str(),
			"durable participant transition",
		)
		.await?;
		Self::lock(tx, id)
			.await?
			.ok_or_else(|| Error::NotFound("participant".into()))
	}

	pub(crate) async fn pending<E: OrmExecutor>(db: &mut E) -> Result<Vec<Self>> {
		Ok(Self::objects()
			.filter(Self::field_phase().is_in([
				AtomicParticipantPhase::Reserved,
				AtomicParticipantPhase::Prepared,
				AtomicParticipantPhase::Applied,
			]))
			.order_by(&["updated_at", "id"])
			.limit(32)
			.all_with_db(db)
			.await?)
	}

	/// Session-local context allows only this participant through its held gate.
	pub(crate) async fn mutation_context(tx: &mut dyn TransactionExecutor, id: Uuid) -> Result<()> {
		let (sql, values) = Query::select()
			.expr(SimpleExpr::FunctionCall(
				"set_config".into_iden(),
				vec![
					Expr::value("aidash.atomic_transaction").into(),
					Expr::value(id.to_string()).into(),
					Expr::value(true).into(),
				],
			))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}
}
