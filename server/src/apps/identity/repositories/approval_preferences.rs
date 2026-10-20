//! A person's Approval Mode and Approval Rules (#411, ADR 0076). Every
//! function runs in the caller's Tenant-scoped transaction. Writers take the
//! owner's advisory lock, so one owner's changes and the revisions they check
//! never race.
use super::super::models::{
	ApprovalPreference, ApprovalRuleRecord, ThreadApprovalMode, database_time,
};
use super::approval_decider::{self, Availability};
use aidash_domain::RunMetadata;
use aidash_domain::approval_rules::{
	ApprovalKind, ApprovalMode, ApprovalRule, RuleEffect, RuleTarget, Settings,
};
use aidash_domain::policy::{PolicyBundle, SubjectKind};
use chrono::{DateTime, Utc};
use reinhardt::db::{
	backends::TransactionExecutor,
	orm::{Model, execution::convert_values},
};
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoIden, JoinType, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SimpleExpr, TableRef,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use uuid::Uuid;

/// Distinct from every other advisory lock namespace in the server.
const OWNER_LOCK: i64 = 408_001;

/// The most rules one person stores, so a rule list stays readable and the
/// per-approval read stays small.
pub(crate) const MAX_RULES: usize = 500;

/// Locks one owner's preferences and rules for the rest of `tx`.
pub(crate) async fn lock_owner(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	subject: &str,
) -> crate::Result<()> {
	let (sql, values) = Query::select()
		.expr(SimpleExpr::FunctionCall(
			"pg_advisory_xact_lock".into_iden(),
			vec![SimpleExpr::FunctionCall(
				"hashtextextended".into_iden(),
				vec![
					// A JSON pair names one owner unambiguously.
					Expr::value(json!([tenant, subject]).to_string()).into(),
					Expr::value(OWNER_LOCK).into(),
				],
			)],
		))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

/// Reads a snake case enum stored as text.
fn stored<T: DeserializeOwned>(what: &str, text: &str) -> crate::Result<T> {
	serde_json::from_value(Value::String(text.to_owned()))
		.map_err(|_| crate::Error::Invalid(format!("stored {what} is invalid: {text}")))
}

/// Writes a snake case enum as the text it is stored as.
pub(crate) fn text(value: &impl serde::Serialize) -> crate::Result<String> {
	match serde_json::to_value(value)? {
		Value::String(text) => Ok(text),
		_ => Err(crate::Error::Invalid("not a text value".into())),
	}
}

/// A stored rule as the domain reads it.
pub(crate) fn rule(row: ApprovalRuleRecord) -> crate::Result<ApprovalRule> {
	Ok(ApprovalRule {
		id: row.id,
		kind: stored::<ApprovalKind>("rule kind", &row.kind)?,
		target: serde_json::from_value::<RuleTarget>(row.target.0)
			.map_err(|_| crate::Error::Invalid("stored rule target is invalid".into()))?,
		effect: stored::<RuleEffect>("rule effect", &row.effect)?,
	})
}

/// The owner's rules in the order they were made, and when each was.
pub(crate) async fn rules(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	subject: &str,
) -> crate::Result<Vec<(ApprovalRule, DateTime<Utc>)>> {
	ApprovalRuleRecord::objects()
		.filter(ApprovalRuleRecord::field_tenant().eq(tenant))
		.filter(ApprovalRuleRecord::field_subject().eq(subject))
		.order_by(&["created_at", "id"])
		.limit(MAX_RULES)
		.all_with_executor(tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.into_iter()
		.map(|row| {
			let created_at = row.created_at;
			rule(row).map(|rule| (rule, created_at))
		})
		.collect()
}

/// Stores `new` for the owner unless the owner already holds an equal rule.
/// Returns whether it was stored. The caller holds [`lock_owner`].
pub(crate) async fn add_rule(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	subject: &str,
	new: &ApprovalRule,
) -> crate::Result<bool> {
	let held = ApprovalRuleRecord::objects()
		.filter(ApprovalRuleRecord::field_tenant().eq(tenant))
		.filter(ApprovalRuleRecord::field_subject().eq(subject))
		.count_with_executor(tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?;
	if held >= MAX_RULES {
		return Err(crate::Error::Conflict(format!(
			"at most {MAX_RULES} Approval Rules are kept"
		)));
	}
	let now = database_time(tx).await?;
	// The row's Tenant is the scope's, which the row-level policy enforces.
	let sql = Query::insert()
		.into_table(Alias::new(ApprovalRuleRecord::table_name()))
		.columns(
			[
				"tenant",
				"id",
				"subject",
				"kind",
				"target",
				"effect",
				"created_at",
			]
			.map(Alias::new),
		)
		.from_subquery(
			Query::select()
				.expr(crate::database::scope::tenant())
				.expr(Expr::value(new.id))
				.expr(Expr::value(subject.to_owned()))
				.expr(Expr::value(text(&new.kind)?))
				.expr(Expr::value(serde_json::to_value(&new.target)?))
				.expr(Expr::value(text(&new.effect)?))
				.expr(Expr::value(now))
				.to_owned(),
		)
		.on_conflict(
			OnConflict::columns(["tenant", "subject", "kind", "effect", "target"].map(Alias::new))
				.do_nothing()
				.to_owned(),
		)
		.to_string(PostgresQueryBuilder);
	Ok(crate::database::native::query(&sql)
		.execute(&mut *tx)
		.await?
		.rows_affected()
		== 1)
}

/// Deletes one of the owner's rules. Returns whether it existed.
pub(crate) async fn delete_rule(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	subject: &str,
	id: Uuid,
) -> crate::Result<bool> {
	let (sql, values) = Query::delete()
		.from_table(Alias::new(ApprovalRuleRecord::table_name()))
		.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant.to_owned())))
		.and_where(Expr::col(Alias::new("subject")).eq(Expr::value(subject.to_owned())))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
		.build(PostgresQueryBuilder);
	Ok(tx
		.execute(&sql, convert_values(values))
		.await?
		.rows_affected
		== 1)
}

/// The owner's default mode and the rest of their preferences.
pub(crate) async fn preference(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	subject: &str,
) -> crate::Result<Option<ApprovalPreference>> {
	Ok(ApprovalPreference::objects()
		.filter(ApprovalPreference::field_tenant().eq(tenant))
		.filter(ApprovalPreference::field_subject().eq(subject))
		.all_with_executor(tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop())
}

/// The mode a preference row stores; Manual without a row.
pub(crate) fn default_mode(row: Option<&ApprovalPreference>) -> crate::Result<ApprovalMode> {
	row.map_or(Ok(ApprovalMode::default()), |row| {
		stored("default mode", &row.default_mode)
	})
}

/// What a write of the owner's preferences stores.
pub(crate) struct PreferenceWrite {
	pub mode: ApprovalMode,
	pub answer_deadline_seconds: Option<i64>,
	/// The owner accepts the risk of Bypass, which `mode` selects, when they
	/// had not before. Asking for it with another mode records nothing.
	pub accept_bypass: bool,
}

/// Stores the owner's preferences at `expected_revision` (0 for the first
/// write) and returns the row. The caller holds [`lock_owner`].
pub(crate) async fn save_preference(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	subject: &str,
	expected_revision: i64,
	write: &PreferenceWrite,
) -> crate::Result<ApprovalPreference> {
	let current = preference(tx, tenant, subject).await?;
	let revision = current.as_ref().map_or(0, |row| row.revision);
	if revision != expected_revision {
		return Err(crate::Error::Conflict(
			"the Approval preferences changed; reload them".into(),
		));
	}
	let now = database_time(tx).await?;
	let accepted = current
		.as_ref()
		.and_then(|row| row.bypass_accepted_at)
		.or_else(|| (write.accept_bypass && write.mode == ApprovalMode::Bypass).then_some(now));
	if write.mode == ApprovalMode::Bypass && accepted.is_none() {
		return Err(crate::Error::Invalid(
			"Bypass needs the person's acceptance of its risk".into(),
		));
	}
	let mode = text(&write.mode)?;
	if current.is_none() {
		let sql = Query::insert()
			.into_table(Alias::new(ApprovalPreference::table_name()))
			.columns(
				[
					"tenant",
					"subject",
					"default_mode",
					"answer_deadline_seconds",
					"bypass_accepted_at",
					"revision",
					"updated_at",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(crate::database::scope::tenant())
					.expr(Expr::value(subject.to_owned()))
					.expr(Expr::value(mode.clone()))
					.expr(Expr::value(write.answer_deadline_seconds))
					.expr(Expr::value(accepted))
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
			.table(Alias::new(ApprovalPreference::table_name()))
			.value_expr(Alias::new("default_mode"), Expr::value(mode.clone()))
			.value_expr(
				Alias::new("answer_deadline_seconds"),
				Expr::value(write.answer_deadline_seconds),
			)
			.value_expr(Alias::new("bypass_accepted_at"), Expr::value(accepted))
			.value_expr(Alias::new("revision"), Expr::cust("revision + 1"))
			.value_expr(Alias::new("updated_at"), Expr::value(now))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant.to_owned())))
			.and_where(Expr::col(Alias::new("subject")).eq(Expr::value(subject.to_owned())))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
	}
	preference(tx, tenant, subject)
		.await?
		.ok_or_else(|| crate::Error::Invalid("Approval preferences are unavailable".into()))
}

/// The owner's mode for one Thread, if they set one.
pub(crate) async fn thread_mode(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	subject: &str,
	thread: Uuid,
) -> crate::Result<Option<ApprovalMode>> {
	ThreadApprovalMode::objects()
		.filter(ThreadApprovalMode::field_tenant().eq(tenant))
		.filter(ThreadApprovalMode::field_subject().eq(subject))
		.filter(ThreadApprovalMode::field_thread_id().eq(thread))
		.all_with_executor(tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop()
		.map(|row| stored("Thread mode", &row.mode))
		.transpose()
}

/// Sets, or with `None` clears, the owner's mode for one Thread. The caller
/// holds [`lock_owner`].
pub(crate) async fn set_thread_mode(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	subject: &str,
	thread: Uuid,
	mode: Option<ApprovalMode>,
) -> crate::Result<()> {
	let held = thread_mode(tx, tenant, subject, thread).await?.is_some();
	let (sql, values) = match (mode, held) {
		(None, _) => Query::delete()
			.from_table(Alias::new(ThreadApprovalMode::table_name()))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant.to_owned())))
			.and_where(Expr::col(Alias::new("subject")).eq(Expr::value(subject.to_owned())))
			.and_where(Expr::col(Alias::new("thread_id")).eq(Expr::value(thread)))
			.build(PostgresQueryBuilder),
		(Some(mode), true) => Query::update()
			.table(Alias::new(ThreadApprovalMode::table_name()))
			.value_expr(Alias::new("mode"), Expr::value(text(&mode)?))
			.value_expr(Alias::new("revision"), Expr::cust("revision + 1"))
			.value_expr(
				Alias::new("updated_at"),
				Expr::value(database_time(tx).await?),
			)
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant.to_owned())))
			.and_where(Expr::col(Alias::new("subject")).eq(Expr::value(subject.to_owned())))
			.and_where(Expr::col(Alias::new("thread_id")).eq(Expr::value(thread)))
			.build(PostgresQueryBuilder),
		(Some(mode), false) => {
			let sql = Query::insert()
				.into_table(Alias::new(ThreadApprovalMode::table_name()))
				.columns(
					[
						"tenant",
						"thread_id",
						"subject",
						"mode",
						"revision",
						"updated_at",
					]
					.map(Alias::new),
				)
				.from_subquery(
					Query::select()
						.expr(crate::database::scope::tenant())
						.expr(Expr::value(thread))
						.expr(Expr::value(subject.to_owned()))
						.expr(Expr::value(text(&mode)?))
						.expr(Expr::value(1_i64))
						.expr(Expr::value(database_time(tx).await?))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder);
			crate::database::native::query(&sql)
				.execute(&mut *tx)
				.await?;
			return Ok(());
		}
	};
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

/// Whether `bundle` lists `subject` as an enabled User of its Tenant: only a
/// person's own mode and rules cover Runs (ADR 0076).
pub(crate) fn is_person(bundle: &PolicyBundle, subject: &str) -> bool {
	bundle
		.subjects
		.get(subject)
		.is_some_and(|entry| entry.enabled && entry.kind == SubjectKind::User)
}

/// The person whose mode covers a Run, and that mode.
pub(crate) struct Covering {
	pub subject: String,
	pub mode: ApprovalMode,
	pub bypass_accepted: bool,
	/// How long the person's Runs wait for an answer to the model's question.
	pub answer_deadline_seconds: Option<i64>,
}

/// Row of the one query that finds who covers a Run and in which mode.
struct Coverage {
	root_subject: String,
	default_mode: Option<String>,
	bypass_accepted_at: Option<DateTime<Utc>>,
	thread_mode: Option<String>,
	answer_deadline_seconds: Option<i64>,
}
crate::native_record!(Coverage {
	root_subject,
	default_mode,
	bypass_accepted_at,
	thread_mode,
	answer_deadline_seconds
});

/// Who covers `run`, and in which mode: the person its delegation chain
/// starts at, in their mode for the Run's Thread, else their default, else
/// Manual. `None` unless the Run executes on its home Node `node` and that
/// person is an enabled User subject of the Tenant, so a Run that arrived
/// through Federation, or that no person started, is never covered. One
/// query, read now, so a lowered ceiling or a changed mode reaches a running
/// Run. `chain_root` is the first subject of the caller's authority: when it
/// is not the person the Run's grant records, nobody is covered.
pub(crate) async fn covering(
	tx: &mut dyn TransactionExecutor,
	bundle: &PolicyBundle,
	node: &str,
	chain_root: Option<&str>,
	run: &RunMetadata,
) -> crate::Result<Option<Covering>> {
	if run.home_node != node {
		return Ok(None);
	}
	let column = |table: &str, name: &str| (Alias::new(table), Alias::new(name));
	let sql = Query::select()
		.expr_as(
			Expr::col(column("e", "root_subject")),
			Alias::new("root_subject"),
		)
		.expr_as(
			Expr::col(column("p", "default_mode")),
			Alias::new("default_mode"),
		)
		.expr_as(
			Expr::col(column("p", "bypass_accepted_at")),
			Alias::new("bypass_accepted_at"),
		)
		.expr_as(Expr::col(column("m", "mode")), Alias::new("thread_mode"))
		.expr_as(
			Expr::col(column("p", "answer_deadline_seconds")),
			Alias::new("answer_deadline_seconds"),
		)
		.from_as(Alias::new("authorization_execution"), Alias::new("e"))
		.join(
			JoinType::LeftJoin,
			TableRef::table_alias(Alias::new("core_task_sessions"), Alias::new("t")),
			Expr::cust("t.task_id = e.task_id"),
		)
		.join(
			JoinType::LeftJoin,
			TableRef::table_alias(
				Alias::new(ApprovalPreference::table_name()),
				Alias::new("p"),
			),
			Expr::cust("p.tenant = e.tenant AND p.subject = e.root_subject"),
		)
		.join(
			JoinType::LeftJoin,
			TableRef::table_alias(
				Alias::new(ThreadApprovalMode::table_name()),
				Alias::new("m"),
			),
			Expr::cust(
				"m.tenant = e.tenant AND m.subject = e.root_subject AND m.thread_id = t.thread_id",
			),
		)
		.and_where(Expr::col(column("e", "run_id")).eq(Expr::value(run.id)))
		.to_string(PostgresQueryBuilder);
	let Some(row) = crate::database::native::query_as::<Coverage>(&sql)
		.fetch_optional(&mut *tx)
		.await?
	else {
		return Ok(None);
	};
	if chain_root != Some(row.root_subject.as_str()) || !is_person(bundle, &row.root_subject) {
		return Ok(None);
	}
	let mode = match row.thread_mode.as_deref().or(row.default_mode.as_deref()) {
		Some(text) => stored("mode", text)?,
		None => ApprovalMode::default(),
	};
	Ok(Some(Covering {
		subject: row.root_subject,
		mode,
		bypass_accepted: row.bypass_accepted_at.is_some(),
		answer_deadline_seconds: row.answer_deadline_seconds,
	}))
}

/// The settings that cover `run`: [`covering`] and the person's rules.
pub(crate) async fn settings_for_run(
	tx: &mut dyn TransactionExecutor,
	bundle: &PolicyBundle,
	tenant: &str,
	node: &str,
	chain_root: Option<&str>,
	run: &RunMetadata,
) -> crate::Result<Option<Settings>> {
	let Some(covering) = covering(tx, bundle, node, chain_root, run).await? else {
		return Ok(None);
	};
	let rules = rules(tx, tenant, &covering.subject).await?;
	let auto_available = auto_available(tx, tenant, covering.mode).await?;
	Ok(Some(Settings {
		mode: covering.mode,
		rules: rules.into_iter().map(|(rule, _)| rule).collect(),
		bypass_accepted: covering.bypass_accepted,
		auto_available,
	}))
}

/// Whether the Tenant's approval Decider can judge on this Node, which only
/// Auto needs to know.
async fn auto_available(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	mode: ApprovalMode,
) -> crate::Result<bool> {
	if mode != ApprovalMode::Auto {
		return Ok(false);
	}
	let chosen = approval_decider::chosen(tx, tenant).await?;
	Ok(approval_decider::availability(tx, tenant, &chosen).await? == Availability::Usable)
}

/// The settings of `root` without a Run: their default mode and rules. `None`
/// unless `root` is an enabled User subject of the Tenant.
pub(crate) async fn settings_for_root(
	tx: &mut dyn TransactionExecutor,
	bundle: &PolicyBundle,
	tenant: &str,
	root: &str,
) -> crate::Result<Option<Settings>> {
	if !is_person(bundle, root) {
		return Ok(None);
	}
	let preference = preference(tx, tenant, root).await?;
	let rules = rules(tx, tenant, root).await?;
	let mode = default_mode(preference.as_ref())?;
	let auto_available = auto_available(tx, tenant, mode).await?;
	Ok(Some(Settings {
		mode,
		rules: rules.into_iter().map(|(rule, _)| rule).collect(),
		bypass_accepted: preference.is_some_and(|row| row.bypass_accepted_at.is_some()),
		auto_available,
	}))
}
