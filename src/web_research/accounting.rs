//! Reserve every HTTP attempt durably before dispatch, including retries and redirects.
use super::{contracts::*, persistence};
use crate::{Error, Result, authorization::access::Access, domain::Run, store::Store};
use chrono::{Duration, Utc};
use sea_orm::sea_query::{Alias, Expr, LockType, OnConflict, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) fn check_attempt_limit(store: &Store, state: &Value, search: bool) -> Result<()> {
	let (field, limit) = if search {
		("search_attempts", store.web.profile.search_attempt_limit)
	} else {
		("page_attempts", store.web.profile.page_attempt_limit)
	};
	if state[field].as_u64().unwrap_or(0) >= limit {
		return Err(Error::Invalid("run_attempt_limit".into()));
	}
	Ok(())
}

pub(crate) async fn reserve(
	store: &Store,
	access: &mut Access,
	run: &Run,
	id: Uuid,
	search: bool,
	host: &str,
	deadline: chrono::DateTime<Utc>,
) -> Result<bool> {
	let now = Utc::now();
	if now >= deadline {
		return Err(Error::Invalid("deadline_exceeded".into()));
	}
	let mut state = persistence::state(access, run).await?;
	if state["revoked"] == true {
		return Err(Error::Forbidden);
	}
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("web_nodes"))
			.columns(["node_id", "data"].map(Alias::new))
			.values_panic([Expr::cust("$1"), Expr::cust("$2")])
			.on_conflict(
				OnConflict::column(Alias::new("node_id"))
					.do_nothing()
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.bind(&store.node_id)
	.bind(json!({"active":[],"next_search_at":now}))
	.execute(&mut **access.tx)
	.await?;
	let mut node: Value = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("data"))
			.from(Alias::new("web_nodes"))
			.and_where(Expr::cust("node_id=$1"))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.bind(&store.node_id)
	.fetch_one(&mut **access.tx)
	.await?;
	let active = node["active"].as_array_mut().ok_or(Error::Forbidden)?;
	active.retain(|entry| persistence::date(&entry["until"]).is_ok_and(|until| until > now));
	let same_kind = active
		.iter()
		.filter(|entry| entry["search"] == search)
		.count();
	if same_kind >= if search { 2 } else { 4 }
		|| (!search
			&& active
				.iter()
				.any(|entry| entry["search"] == false && entry["host"] == host))
		|| (search && persistence::date(&node["next_search_at"])? > now)
	{
		return Ok(false);
	}
	let field = if search {
		"search_attempts"
	} else {
		"page_attempts"
	};
	let attempts = state[field].as_u64().unwrap_or(0);
	check_attempt_limit(store, &state, search)?;
	let mut cost = 0;
	if search {
		let account = store
			.web
			.account
			.as_ref()
			.ok_or_else(|| Error::Invalid("account_unavailable".into()))?;
		account.validate_at(now)?;
		if !crate::web_search::credential_available(account) {
			return Err(Error::Invalid("account_unavailable".into()));
		}
		let month = now.format("%Y-%m").to_string();
		let base = i64::try_from(account.monthly_base_fee_micro_usd)
			.map_err(|_| Error::Invalid("invalid_tariff".into()))?;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("web_months"))
				.columns(
					[
						"node_id",
						"account_id",
						"month",
						"used_micro_usd",
						"base_micro_usd",
					]
					.map(Alias::new),
				)
				.values_panic([
					Expr::cust("$1"),
					Expr::cust("$2"),
					Expr::cust("$3"),
					Expr::cust("$4"),
					Expr::cust("$4"),
				])
				.on_conflict(
					OnConflict::columns(["node_id", "account_id", "month"].map(Alias::new))
						.do_nothing()
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.bind(&store.node_id)
		.bind(&account.account_id)
		.bind(&month)
		.bind(base)
		.execute(&mut **access.tx)
		.await?;
		let (used, old_base): (i64, i64) = sqlx::query_as(
			&Query::select()
				.columns(["used_micro_usd", "base_micro_usd"].map(Alias::new))
				.from(Alias::new("web_months"))
				.and_where(Expr::cust("node_id=$1 AND account_id=$2 AND month=$3"))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.bind(&store.node_id)
		.bind(&account.account_id)
		.bind(&month)
		.fetch_one(&mut **access.tx)
		.await?;
		cost = account.request_price_micro_usd;
		// A tariff change can raise this month's recurring fee, never refund an
		// already reserved attempt or reset a month's accumulated estimate.
		let next = (used as u64)
			.checked_add((base - old_base).max(0) as u64)
			.and_then(|n| n.checked_add(cost))
			.ok_or_else(|| Error::Invalid("monthly_limit".into()))?;
		let node_used: i64 = sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COALESCE(SUM(used_micro_usd),0)::bigint"))
				.from(Alias::new("web_months"))
				.and_where(Expr::cust("node_id=$1 AND month=$2"))
				.to_string(PostgresQueryBuilder),
		)
		.bind(&store.node_id)
		.bind(&month)
		.fetch_one(&mut **access.tx)
		.await?;
		let node_next = (node_used as u64)
			.checked_sub(used as u64)
			.and_then(|total| total.checked_add(next));
		if node_next.is_none_or(|total| total > account.monthly_limit_micro_usd.min(20_000_000))
			|| next > i64::MAX as u64
		{
			return Err(Error::Invalid("monthly_limit".into()));
		}
		sqlx::query(
			&Query::update()
				.table(Alias::new("web_months"))
				.value(Alias::new("used_micro_usd"), Expr::cust("$4"))
				.value(
					Alias::new("base_micro_usd"),
					Expr::cust("GREATEST(base_micro_usd,$5)"),
				)
				.and_where(Expr::cust("node_id=$1 AND account_id=$2 AND month=$3"))
				.to_string(PostgresQueryBuilder),
		)
		.bind(&store.node_id)
		.bind(&account.account_id)
		.bind(month)
		.bind(next as i64)
		.bind(base)
		.execute(&mut **access.tx)
		.await?;
		state["account_id"] = json!(account.account_id);
		state["price_effective_at"] = json!(account.price_effective_at);
		state["last_month"] = json!(now.format("%Y-%m").to_string());
		node["next_search_at"] = json!(now + Duration::seconds(1));
	}
	state[field] = json!(attempts + 1);
	state["estimated_micro_usd"] = json!(state["estimated_micro_usd"].as_u64().unwrap_or(0) + cost);
	persistence::save_state(access, run, &state).await?;
	node["active"]
		.as_array_mut()
		.ok_or(Error::Forbidden)?
		.push(json!({"id":id,"search":search,"host":host,"until":deadline}));
	sqlx::query(
		&Query::update()
			.table(Alias::new("web_nodes"))
			.value(Alias::new("data"), Expr::cust("$2"))
			.and_where(Expr::cust("node_id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(&store.node_id)
	.bind(node)
	.execute(&mut **access.tx)
	.await?;
	Ok(true)
}
pub(crate) async fn release(store: &Store, id: Uuid) -> Result<()> {
	// The original deadline also releases abandoned slots after a crash. The
	// debit remains durable even when no result was received.
	sqlx::query(&Query::update().table(Alias::new("web_nodes"))
		.value(Alias::new("data"), Expr::cust("jsonb_set(data,'{active}',COALESCE((SELECT jsonb_agg(e) FROM jsonb_array_elements(data->'active') AS e WHERE e->>'id'<>$2),'[]'::jsonb))"))
		.and_where(Expr::cust("node_id=$1")).to_string(PostgresQueryBuilder)).bind(&store.node_id).bind(id.to_string())
		.execute(&store.pool).await?;
	Ok(())
}
pub(crate) async fn observation(access: &mut Access, run: &Run, bytes: usize) -> Result<()> {
	let mut state = persistence::state(access, run).await?;
	let count = state["observation_count"].as_u64().unwrap_or(0);
	let used = state["observation_bytes"].as_u64().unwrap_or(0);
	if count >= MAX_OBSERVATIONS || used.saturating_add(bytes as u64) > MAX_OBSERVATION_BYTES {
		return Err(Error::Invalid("observation_limit".into()));
	}
	state["observation_count"] = json!(count + 1);
	state["observation_bytes"] = json!(used + bytes as u64);
	persistence::save_state(access, run, &state).await
}
