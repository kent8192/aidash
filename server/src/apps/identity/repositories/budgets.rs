//! Budgets, their Soft Limit notices and the spend ledger (#1012, ADR 0143).
use crate::database::native::{self, Pool};
use crate::{Error, Result};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::budgets::{self as domain, Definition, Gate, OnHardLimit, Period, Scope};
use chrono::{DateTime, Utc};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::{
	Alias, Expr, ExprTrait, OnConflict, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use uuid::Uuid;

/// Spend that belongs to no Duty is filed under the nil UUID.
const NO_DUTY: Uuid = Uuid::nil();

/// One Budget as stored.
#[derive(Debug, Clone)]
pub(crate) struct Budget {
	pub id: Uuid,
	pub tenant: String,
	pub definition: Definition,
	pub revision: i64,
	pub created_at: DateTime<Utc>,
	pub created_by: String,
	pub updated_at: DateTime<Utc>,
}

const COLUMNS: [&str; 13] = [
	"id",
	"tenant",
	"scope_kind",
	"scope_key",
	"period",
	"soft_limit_nanocredits",
	"hard_limit_nanocredits",
	"on_hard_limit",
	"recipients",
	"revision",
	"created_at",
	"created_by",
	"updated_at",
];

fn from_row(row: &native::Row) -> Result<Budget> {
	let text = |name: &str| -> Result<String> { row.try_get(name) };
	Ok(Budget {
		id: row.try_get("id")?,
		tenant: text("tenant")?,
		definition: Definition {
			scope: Scope::from_parts(&text("scope_kind")?, &text("scope_key")?)?,
			period: Period::parse(&text("period")?)?,
			soft_limit_nanocredits: row.try_get("soft_limit_nanocredits")?,
			hard_limit_nanocredits: row.try_get("hard_limit_nanocredits")?,
			on_hard_limit: OnHardLimit::parse(&text("on_hard_limit")?)?,
			recipients: row.try_get("recipients")?,
		},
		revision: row.try_get("revision")?,
		created_at: row.try_get("created_at")?,
		created_by: text("created_by")?,
		updated_at: row.try_get("updated_at")?,
	})
}

fn select() -> reinhardt::query::SelectStatement {
	let mut query = Query::select();
	query
		.columns(COLUMNS.map(Alias::new))
		.from(Alias::new("budgets"));
	query
}

pub(crate) async fn list(tx: &mut dyn TransactionExecutor, tenant: &str) -> Result<Vec<Budget>> {
	native::query(
		&select()
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.order_by(Alias::new("created_at"), Order::Asc)
			.order_by(Alias::new("id"), Order::Asc)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut *tx)
	.await?
	.iter()
	.map(from_row)
	.collect()
}

pub(crate) async fn get(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	id: Uuid,
	lock: bool,
) -> Result<Budget> {
	let mut query = select();
	query
		.and_where(Expr::col("tenant").eq(tenant.to_owned()))
		.and_where(Expr::col("id").eq(Expr::value(id)));
	if lock {
		query.lock(reinhardt::query::LockType::Update);
	}
	let row = native::query(&query.to_string(PostgresQueryBuilder))
		.fetch_optional(&mut *tx)
		.await?
		.ok_or_else(|| Error::NotFound("budget not found".into()))?;
	from_row(&row)
}

pub(crate) async fn insert(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	definition: &Definition,
	by: &str,
) -> Result<Budget> {
	let id = Uuid::now_v7();
	let now = Utc::now();
	let inserted = native::query(
		&Query::insert()
			.into_table(Alias::new("budgets"))
			.columns(COLUMNS.map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::value(id))
					.expr(Expr::value(tenant.to_owned()))
					.expr(Expr::value(definition.scope.kind()))
					.expr(Expr::value(definition.scope.key()))
					.expr(Expr::value(definition.period.as_str()))
					.expr(Expr::value(definition.soft_limit_nanocredits))
					.expr(Expr::value(definition.hard_limit_nanocredits))
					.expr(Expr::value(definition.on_hard_limit.as_str()))
					.expr(crate::database::text_array(definition.recipients.clone()))
					.expr(Expr::value(1_i64))
					.expr(Expr::value(now))
					.expr(Expr::value(by.to_owned()))
					.expr(Expr::value(now))
					.to_owned(),
			)
			.on_conflict(
				OnConflict::columns(["tenant", "scope_kind", "scope_key"].map(Alias::new))
					.do_nothing()
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?
	.rows_affected();
	if inserted == 0 {
		return Err(Error::Conflict(
			"this scope already has a Budget; change that one".into(),
		));
	}
	get(tx, tenant, id, false).await
}

pub(crate) async fn update(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	id: Uuid,
	definition: &Definition,
	expected_revision: i64,
	by: &str,
) -> Result<Budget> {
	let current = get(tx, tenant, id, true).await?;
	if current.revision != expected_revision {
		return Err(Error::Conflict("budget revision changed".into()));
	}
	if current.definition.scope != definition.scope {
		return Err(Error::Invalid("a Budget's scope cannot change".into()));
	}
	native::query(
		&Query::update()
			.table(Alias::new("budgets"))
			.value_expr(
				Alias::new("period"),
				Expr::value(definition.period.as_str()),
			)
			.value_expr(
				Alias::new("soft_limit_nanocredits"),
				Expr::value(definition.soft_limit_nanocredits),
			)
			.value_expr(
				Alias::new("hard_limit_nanocredits"),
				Expr::value(definition.hard_limit_nanocredits),
			)
			.value_expr(
				Alias::new("on_hard_limit"),
				Expr::value(definition.on_hard_limit.as_str()),
			)
			.value_expr(
				Alias::new("recipients"),
				crate::database::text_array(definition.recipients.clone()),
			)
			.value_expr(Alias::new("revision"), Expr::value(current.revision + 1))
			// Whoever sets a Budget last is who may change it without
			// `budget.manage` (see the service).
			.value_expr(Alias::new("created_by"), Expr::value(by.to_owned()))
			.value_expr(Alias::new("updated_at"), Expr::value(Utc::now()))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	get(tx, tenant, id, false).await
}

pub(crate) async fn delete(tx: &mut dyn TransactionExecutor, tenant: &str, id: Uuid) -> Result<()> {
	let deleted = native::query(
		&Query::delete()
			.from_table(Alias::new("budgets"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?
	.rows_affected();
	if deleted == 0 {
		return Err(Error::NotFound("budget not found".into()));
	}
	Ok(())
}

/// Spend and calls without a reported cost in a Budget's current window.
pub(crate) async fn spent(
	tx: &mut dyn TransactionExecutor,
	budget: &Budget,
	now: DateTime<Utc>,
) -> Result<(i64, i64)> {
	let since = budget
		.definition
		.period
		.window_start(budget.created_at, now);
	// The ledger is daily: a Total Budget counts its whole first day.
	let since = since
		.date_naive()
		.and_hms_opt(0, 0, 0)
		.expect("midnight")
		.and_utc();
	let mut query = Query::select();
	query
		.expr_as(
			Expr::cust("COALESCE(SUM(spent_nanocredits), 0)::bigint"),
			Alias::new("spent"),
		)
		.expr_as(
			Expr::cust("COALESCE(SUM(unpriced_calls), 0)::bigint"),
			Alias::new("unpriced"),
		)
		.from(Alias::new("budget_spend"))
		.and_where(Expr::col("tenant").eq(budget.tenant.clone()))
		.and_where(Expr::col("day").gte(Expr::value(since)));
	match &budget.definition.scope {
		Scope::Tenant => {}
		Scope::Node { node } => {
			query.and_where(Expr::col("node").eq(node.clone()));
		}
		Scope::Duty { duty_id } => {
			query.and_where(Expr::col("duty_id").eq(Expr::value(*duty_id)));
		}
		Scope::Person { subject } => {
			query.and_where(Expr::col("root_subject").eq(subject.clone()));
		}
	}
	let row = native::query(&query.to_string(PostgresQueryBuilder))
		.fetch_one(&mut *tx)
		.await?;
	Ok((row.try_get("spent")?, row.try_get("unpriced")?))
}

/// What a model call by `run` is charged to.
pub(crate) struct Charge {
	pub tenant: String,
	pub node: String,
	pub root_subject: String,
	pub duty: Uuid,
}

async fn charge_of(tx: &mut dyn TransactionExecutor, run: Uuid) -> Result<Option<Charge>> {
	let row = native::query(
		&Query::select()
			.column((Alias::new("r"), Alias::new("tenant")))
			.column((Alias::new("r"), Alias::new("node")))
			.expr_as(
				SimpleExpr::CustomWithExpr(
					"COALESCE((SELECT e.root_subject FROM authorization_execution e WHERE e.run_id = r.id), '')".to_owned(),
					vec![],
				),
				Alias::new("root_subject"),
			)
			.expr_as(
				SimpleExpr::CustomWithExpr(
					"COALESCE((SELECT t.duty_id FROM tasks t WHERE t.id = r.task_id AND t.tenant = r.tenant), ?::uuid)".to_owned(),
					vec![Expr::value(NO_DUTY).into()],
				),
				Alias::new("duty_id"),
			)
			.from_as(Alias::new("task_executions"), Alias::new("r"))
			.and_where(Expr::col((Alias::new("r"), Alias::new("id"))).eq(Expr::value(run)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut *tx)
	.await?;
	row.map(|row| {
		Ok(Charge {
			tenant: row.try_get("tenant")?,
			node: row.try_get("node")?,
			root_subject: row.try_get("root_subject")?,
			duty: row.try_get("duty_id")?,
		})
	})
	.transpose()
}

/// Adds one model call to the ledger: its reported cost, or one unpriced call.
pub(crate) async fn record_spend(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
	cost: Option<i64>,
	now: DateTime<Utc>,
) -> Result<()> {
	let Some(charge) = charge_of(tx, run).await? else {
		return Ok(());
	};
	let day = now
		.date_naive()
		.and_hms_opt(0, 0, 0)
		.expect("midnight")
		.and_utc();
	let key = [
		("tenant", Expr::value(charge.tenant.clone())),
		("day", Expr::value(day)),
		("node", Expr::value(charge.node.clone())),
		("root_subject", Expr::value(charge.root_subject.clone())),
		("duty_id", Expr::value(charge.duty)),
	];
	native::query(
		&Query::insert()
			.into_table(Alias::new("budget_spend"))
			.columns(
				[
					"tenant",
					"day",
					"node",
					"root_subject",
					"duty_id",
					"spent_nanocredits",
					"unpriced_calls",
				]
				.map(Alias::new),
			)
			.from_subquery({
				let mut row = Query::select();
				for (_, value) in &key {
					row.expr(value.clone());
				}
				row.expr(Expr::value(0_i64))
					.expr(Expr::value(0_i64))
					.to_owned()
			})
			.on_conflict(
				OnConflict::columns(
					["tenant", "day", "node", "root_subject", "duty_id"].map(Alias::new),
				)
				.do_nothing()
				.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	let mut update = Query::update();
	update.table(Alias::new("budget_spend"));
	match cost {
		Some(cost) => {
			update.value_expr(
				Alias::new("spent_nanocredits"),
				Expr::col("spent_nanocredits").add(Expr::value(cost)),
			);
		}
		None => {
			update.value_expr(
				Alias::new("unpriced_calls"),
				Expr::col("unpriced_calls").add(Expr::value(1_i64)),
			);
		}
	}
	for (column, value) in key {
		update.and_where(Expr::col(column).eq(value));
	}
	native::query(&update.to_string(PostgresQueryBuilder))
		.execute(&mut *tx)
		.await?;
	Ok(())
}

/// The Budgets with a Hard Limit that cover a Run, and the gate they set on
/// its next model call.
pub(crate) async fn gate(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
	now: DateTime<Utc>,
) -> Result<(Gate, Vec<Budget>)> {
	let Some(charge) = charge_of(tx, run).await? else {
		return Ok((Gate::Open, vec![]));
	};
	gate_for(tx, &charge, now).await
}

/// The gate the Budgets set on work charged as `charge`; a Duty asks it
/// before starting a Task (#1012 decision 33).
pub(crate) async fn gate_for(
	tx: &mut dyn TransactionExecutor,
	charge: &Charge,
	now: DateTime<Utc>,
) -> Result<(Gate, Vec<Budget>)> {
	let mut covering = vec![];
	for budget in list(tx, &charge.tenant).await? {
		if budget.definition.hard_limit_nanocredits.is_none() {
			continue;
		}
		let covers = match &budget.definition.scope {
			Scope::Tenant => true,
			Scope::Node { node } => *node == charge.node,
			Scope::Duty { duty_id } => *duty_id == charge.duty,
			Scope::Person { subject } => *subject == charge.root_subject,
		};
		if covers {
			let (spent, _) = spent(tx, &budget, now).await?;
			covering.push((budget, spent));
		}
	}
	let gate = domain::gate(
		covering
			.iter()
			.map(|(budget, spent)| (budget.id, &budget.definition, *spent)),
		now,
	);
	Ok((
		gate,
		covering.into_iter().map(|(budget, _)| budget).collect(),
	))
}

/// Every Budget with a Soft Limit, across Tenants, for the notice pass.
pub(crate) async fn with_soft_limits(pool: &Pool) -> Result<Vec<Budget>> {
	let scope = TenancyScope::Deployment(DeploymentScopeReason::Retention);
	pool.scoped(&scope, async |tx| {
		native::query(
			&select()
				.and_where(Expr::col("soft_limit_nanocredits").is_not_null())
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(&mut **tx)
		.await?
		.iter()
		.map(from_row)
		.collect()
	})
	.await
}

/// Claims the Soft Limit notice of one window; false when it was sent.
pub(crate) async fn claim_notice(
	tx: &mut dyn TransactionExecutor,
	budget: &Budget,
	window_start: DateTime<Utc>,
	now: DateTime<Utc>,
) -> Result<bool> {
	Ok(native::query(
		&Query::insert()
			.into_table(Alias::new("budget_notices"))
			.columns(["budget_id", "window_start", "tenant", "sent_at"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::value(budget.id))
					.expr(Expr::value(window_start))
					.expr(Expr::value(budget.tenant.clone()))
					.expr(Expr::value(now))
					.to_owned(),
			)
			.on_conflict(
				OnConflict::columns(["budget_id", "window_start"].map(Alias::new))
					.do_nothing()
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?
	.rows_affected()
		== 1)
}

/// Removes the Budgets of a Duty with the Duty; its spend stays in the ledger.
pub(crate) async fn delete_for_duty(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	duty: Uuid,
) -> Result<()> {
	native::query(
		&Query::delete()
			.from_table(Alias::new("budgets"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("scope_kind").eq("duty"))
			.and_where(Expr::col("scope_key").eq(duty.to_string()))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	Ok(())
}
