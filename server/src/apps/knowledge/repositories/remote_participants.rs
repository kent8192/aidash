//! A foreign generated Agent receives a fresh Home logical identity, without copying a template's units.
use super::{access::Lease, bank_settings, native_memory, units};
use crate::{Error, Result, database::native};
use aidash_domain::{memory::Bank, registry::EntityRef, semantic::remote::NativeOrigin};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, LockType, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use serde_json::json;

pub(crate) async fn issue(
	lease: &mut Lease<'_>,
	template: &Bank,
	agent: &EntityRef,
	provider: &EntityRef,
	origin: &NativeOrigin,
) -> Result<Bank> {
	let id = crate::semantic::native_memory::request_id(
		origin.intent_id,
		&serde_json::to_string(
			&json!({"purpose":"generated-home-logical-agent","home":template.home,"tenant":template.tenant,"workspace":template.workspace,"template":template.participant,"executor":origin.node_id}),
		)?,
	)?;
	let mut bank = template.clone();
	bank.participant = Some(id);
	let existing = native::query(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("memory_participants"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?;
	if existing.is_none() {
		units::authorize(lease, template, "memory.participant.create").await?;
		let principal = lease.saved()?["subject"]
			.as_str()
			.ok_or(Error::Forbidden)?
			.to_owned();
		native::query(
			&Query::insert()
				.into_table(Alias::new("memory_participants"))
				.columns(
					[
						"id",
						"home",
						"tenant",
						"workspace_id",
						"principal",
						"agent_id",
						"agent_version",
						"revision",
						"deleted",
					]
					.map(Alias::new),
				)
				.from_subquery(
					Query::select()
						.expr(Expr::value(id))
						.expr(Expr::value(&bank.home))
						.expr(Expr::value(&bank.tenant))
						.expr(Expr::value(bank.workspace))
						.expr(Expr::value(principal))
						.expr(Expr::value(&agent.id))
						.expr(Expr::value(&agent.version))
						.expr(Expr::value(1_i64))
						.expr(Expr::value(false))
						.to_owned(),
				)
				.on_conflict(OnConflict::column(Alias::new("id")).do_nothing().to_owned())
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **lease.tx())
		.await?;
		native_memory::bank_id(lease, &bank, true).await?;
		bank_settings::ensure(lease, &bank, provider).await?;
	}
	let row = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_participants"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut **lease.tx())
	.await?;
	if row.try_get::<bool>("deleted")?
		|| row.try_get::<String>("home")? != bank.home
		|| row.try_get::<String>("tenant")? != bank.tenant
		|| row.try_get::<uuid::Uuid>("workspace_id")? != bank.workspace
		|| row.try_get::<String>("agent_id")? != agent.id
		|| row.try_get::<String>("agent_version")? != agent.version
	{
		return Err(Error::Conflict("generated Home participant changed".into()));
	}
	Ok(bank)
}
