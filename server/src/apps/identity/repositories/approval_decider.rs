//! The approval Decider a Tenant chose for the Auto Approval Mode (#412, ADR
//! 0077). Every function runs in the caller's Tenant-scoped transaction.
use super::super::models::{TenantApprovalSettings, database_time};
use crate::apps::registry::models::Definition;
use aidash_application::registry::system::{APPROVAL_DECIDER_ID, APPROVAL_DECIDER_VERSION};
use aidash_domain::{
	decision::{DeciderConfig, Hook},
	registry::{EntityRef, Entry},
};
use reinhardt::db::{
	backends::TransactionExecutor,
	orm::{Model, execution::convert_values},
};
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoIden, JoinType, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr, TableRef,
};

/// Distinct from every other advisory lock namespace in the server.
const DECIDER_LOCK: i64 = 412_001;

/// Locks the Tenant's approval Decider setting for the rest of `tx`, so two
/// changes never both start from one revision.
pub(crate) async fn lock(tx: &mut dyn TransactionExecutor, tenant: &str) -> crate::Result<()> {
	let (sql, values) = Query::select()
		.expr(SimpleExpr::FunctionCall(
			"pg_advisory_xact_lock".into_iden(),
			vec![SimpleExpr::FunctionCall(
				"hashtextextended".into_iden(),
				vec![
					Expr::value(tenant.to_owned()).into(),
					Expr::value(DECIDER_LOCK).into(),
				],
			)],
		))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

/// The built-in approval Decider, which a Tenant uses until it names another.
pub(crate) fn built_in() -> EntityRef {
	EntityRef {
		id: APPROVAL_DECIDER_ID.into(),
		version: APPROVAL_DECIDER_VERSION.into(),
	}
}

/// The approval Decider of a Tenant and where it came from.
pub(crate) struct Chosen {
	pub reference: EntityRef,
	/// The Tenant has named no other, and the built-in is used.
	pub built_in: bool,
	/// `0` until the Tenant first names a Decider or resets to the built-in.
	pub revision: i64,
}

/// Whether the chosen Decider can judge on this Node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Availability {
	/// A Decider of hook `approval` that holds an enabled Catalog Approval.
	Usable,
	/// A Decider of hook `approval` with no Catalog Approval on this Node.
	Unapproved,
	/// No Decider of hook `approval` is registered under the chosen name.
	Missing,
}

async fn settings(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
) -> crate::Result<Option<TenantApprovalSettings>> {
	Ok(TenantApprovalSettings::objects()
		.filter(TenantApprovalSettings::field_tenant().eq(tenant))
		.all_with_executor(tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop())
}

/// The Decider the Tenant chose, or the built-in one.
pub(crate) async fn chosen(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
) -> crate::Result<Chosen> {
	let row = settings(tx, tenant).await?;
	let revision = row.as_ref().map_or(0, |row| row.revision);
	Ok(
		match row.and_then(|row| Some((row.decider_id?, row.decider_version?))) {
			Some((id, version)) => Chosen {
				reference: EntityRef { id, version },
				built_in: false,
				revision,
			},
			None => Chosen {
				reference: built_in(),
				built_in: true,
				revision,
			},
		},
	)
}

/// The Decider's definition in the Tenant's Registry when it is one of hook
/// `approval`.
pub(crate) async fn registered(
	tx: &mut dyn TransactionExecutor,
	reference: &EntityRef,
) -> crate::Result<Option<Entry>> {
	let entry = match Definition::read_in(tx, &reference.id, &reference.version).await {
		Ok(entry) => entry,
		Err(crate::Error::NotFound(_)) => return Ok(None),
		Err(error) => return Err(error),
	};
	Ok(is_approval_decider(&entry).then_some(entry))
}

/// An entry of kind `decider` whose hook is `approval`.
pub(crate) fn is_approval_decider(entry: &Entry) -> bool {
	entry.kind == "decider"
		&& serde_json::from_value::<DeciderConfig>(entry.config.clone())
			.is_ok_and(|config| config.hook == Hook::Approval)
}

/// Whether the exact version holds an enabled Catalog Approval on `node`,
/// whatever any subject may do with it.
pub(crate) async fn approved(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &aidash_domain::nodes::NodeName,
	reference: &EntityRef,
) -> crate::Result<bool> {
	let column = |table: &str, name: &str| (Alias::new(table), Alias::new(name));
	let (sql, values) = Query::select()
		.expr(Expr::cust("1"))
		.from_as(Alias::new("authorization_catalog"), Alias::new("c"))
		.join(
			JoinType::InnerJoin,
			TableRef::table_alias(Alias::new("registry"), Alias::new("r")),
			Expr::cust("r.tenant = c.tenant AND r.id = c.entry_id AND r.version = c.entry_version"),
		)
		.and_where(Expr::col(column("c", "tenant")).eq(Expr::value(tenant.to_owned())))
		.and_where(Expr::col(column("c", "node")).eq(Expr::value(node.as_str())))
		.and_where(Expr::col(column("c", "entry_id")).eq(Expr::value(reference.id.clone())))
		.and_where(
			Expr::col(column("c", "entry_version")).eq(Expr::value(reference.version.clone())),
		)
		.and_where(Expr::col(column("c", "enabled")))
		.limit(1)
		.build(PostgresQueryBuilder);
	Ok(tx
		.fetch_optional(&sql, convert_values(values))
		.await?
		.is_some())
}

/// Whether the Tenant's chosen Decider can judge on this Node now.
pub(crate) async fn availability(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &aidash_domain::nodes::NodeName,
	chosen: &Chosen,
) -> crate::Result<Availability> {
	if registered(tx, &chosen.reference).await?.is_none() {
		return Ok(Availability::Missing);
	}
	Ok(if approved(tx, tenant, node, &chosen.reference).await? {
		Availability::Usable
	} else {
		Availability::Unapproved
	})
}

/// Names `decider` as the Tenant's approval Decider, or with `None` resets it to
/// the built-in, when the setting is at `expected_revision`. The caller has
/// checked that `decider` is registered and the actor's authority.
pub(crate) async fn save(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	expected_revision: i64,
	decider: Option<&EntityRef>,
) -> crate::Result<TenantApprovalSettings> {
	let current = settings(tx, tenant).await?;
	let revision = current.as_ref().map_or(0, |row| row.revision);
	if revision != expected_revision {
		return Err(crate::Error::Conflict(
			"the approval Decider changed; reload it".into(),
		));
	}
	let now = database_time(tx).await?;
	let (id, version) = decider.map_or((None, None), |decider| {
		(Some(decider.id.clone()), Some(decider.version.clone()))
	});
	if current.is_none() {
		let sql = Query::insert()
			.into_table(Alias::new(TenantApprovalSettings::table_name()))
			.columns(
				[
					"tenant",
					"decider_id",
					"decider_version",
					"revision",
					"updated_at",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(crate::database::scope::tenant())
					.expr(Expr::value(id))
					.expr(Expr::value(version))
					.expr(Expr::value(1_i64))
					.expr(Expr::value(now))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder);
		crate::database::native::query(&sql)
			.execute(&mut *tx)
			.await?;
	} else {
		let (sql, values) = Query::update()
			.table(Alias::new(TenantApprovalSettings::table_name()))
			.value_expr(Alias::new("decider_id"), Expr::value(id))
			.value_expr(Alias::new("decider_version"), Expr::value(version))
			.value_expr(Alias::new("revision"), Expr::cust("revision + 1"))
			.value_expr(Alias::new("updated_at"), Expr::value(now))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant.to_owned())))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
	}
	settings(tx, tenant)
		.await?
		.ok_or_else(|| crate::Error::Invalid("the approval Decider is unavailable".into()))
}
