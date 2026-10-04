//! Management summaries deliberately exclude query and source text.
use super::{Binding, Failure, Receipt, journal::Record};
use crate::apps::execution::generation::repositories::contracts::RequestAccess;
use crate::{
	Error,
	authorization::{access::Access, identity::Actor},
	federation::Federation,
};
use crate::{Result, store::Store};

use chrono::{DateTime, Utc};
use reinhardt::query::{Alias, Expr, Order, PostgresQueryBuilder, Query};
use serde::{Deserialize, Serialize};

use uuid::Uuid;

#[derive(Debug, Serialize, schemars::JsonSchema)]
#[schemars(rename = "RemoteSemanticSourceProvenance")]
pub struct SourceProvenance {
	pub entry_id: Uuid,
	pub revision: i64,
	pub content_digest: String,
	pub agent: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
#[schemars(rename = "RemoteSemanticProvenance")]
pub struct Provenance {
	pub home_node: String,
	pub operation_id: Uuid,
	pub executor: String,
	pub binding: Binding,
	pub model: String,
	pub model_version: String,
	pub retrieved_at: DateTime<Utc>,
	pub truncated: bool,
	pub sources: Vec<SourceProvenance>,
	/// Current, authorized counters at the node serving this view. Foreign
	/// balances are never presented as authoritative cached counters.
	pub allowance_node: String,
	pub allowances: Vec<Allowance>,
}

#[derive(Debug, Serialize, sqlx::FromRow, schemars::JsonSchema)]
#[schemars(rename = "RemoteSemanticAllowance")]
pub struct Allowance {
	pub request_id: Uuid,
	pub token_limit: i64,
	pub used_tokens: i64,
	pub embedding_call_limit: i64,
	pub embedding_calls: i64,
	pub compaction_call_limit: i64,
	pub compaction_calls: i64,
}
impl From<Receipt> for Provenance {
	fn from(receipt: Receipt) -> Self {
		Self {
			allowance_node: String::new(),
			allowances: vec![],
			home_node: receipt.home_node,
			operation_id: receipt.operation_id,
			executor: receipt.executor,
			binding: receipt.binding,
			model: receipt.result.model,
			model_version: receipt.result.model_version,
			retrieved_at: receipt.retrieved_at,
			truncated: receipt.query_truncated || receipt.result.truncated,
			sources: receipt
				.sources
				.into_iter()
				.zip(receipt.result.matches)
				.map(|(source, found)| SourceProvenance {
					entry_id: source.entry_id,
					revision: source.revision,
					content_digest: source.content_digest,
					agent: found.agent,
				})
				.collect(),
		}
	}
}

pub(crate) async fn provenance(
	access: &mut Access,
	node: &str,
	value: Option<serde_json::Value>,
) -> Result<Option<Provenance>> {
	let Some(value) = value else { return Ok(None) };
	let mut result = Provenance::from(serde_json::from_value::<Receipt>(value)?);
	result.allowance_node = node.into();
	if let Binding::RequiredHome {
		home_lineage,
		execution_lineage,
		..
	} = &result.binding
	{
		for owner in home_lineage
			.iter()
			.chain(execution_lineage)
			.filter(|owner| owner.node_id == node)
		{
			if owner.tenant != access.identity.tenant {
				continue;
			}
			let job: Option<crate::generation::Request> = {
				let query_bind_1 = owner.request_id;
				let query_bind_2 = &owner.tenant;
				crate::database::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("generation_requests"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=? AND tenant=?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
							],
						))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **access.tx)
				.await?
			};
			if let Some(job) = job
				&& job.visible(access).await?
			{
				let usage = {
					let query_bind_1 = owner.request_id;
					sqlx::query_as(
						&Query::select()
							.column(Asterisk)
							.from(Alias::new("generation_budgets"))
							.and_where(SimpleExpr::CustomWithExpr(
								"(request_id=?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.to_string(PostgresQueryBuilder),
					)
					.fetch_one(&mut **access.tx)
					.await?
				};
				result.allowances.push(usage);
			}
		}
	}
	Ok(Some(result))
}

pub(crate) async fn run_receipt(
	f: Federation,
	actor: Actor,
	id: Uuid,
) -> Result<Option<Provenance>> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = async {
		let run = f.store.run(id).await?;
		if !access.run_visible(&run).await? {
			return Err(Error::Forbidden);
		}
		let receipt: Option<serde_json::Value> = {
			let query_bind_1 = id;
			sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("receipt"))
					.from(Alias::new("semantic_remote_receipts"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(run_id=?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.order_by(Alias::new("created_at"), Order::Desc)
					.order_by(Alias::new("operation_id"), Order::Desc)
					.limit(1)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **access.tx)
			.await?
		};
		provenance(&mut access, &f.config.node_id, receipt).await
	}
	.await;
	access.finish(result).await
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "RemoteSemanticStatus")]
pub struct Status {
	pub state: String,
	pub reason: Option<Failure>,
	pub operation_id: Option<Uuid>,
	pub retry_count: i32,
	pub retry_at: Option<DateTime<Utc>>,
	pub result_count: Option<usize>,
	pub truncated: bool,
	pub retrieved_at: Option<DateTime<Utc>>,
}

pub(crate) async fn load(
	store: &Store,
	grant: Uuid,
	binding: &Binding,
	reason: Option<Failure>,
) -> Result<Status> {
	let mut result = Status {
		state: if binding.disabled() {
			"disabled"
		} else {
			"pending"
		}
		.into(),
		reason,
		operation_id: None,
		retry_count: 0,
		retry_at: None,
		result_count: None,
		truncated: false,
		retrieved_at: None,
	};
	if binding.disabled() {
		return Ok(result);
	}
	let record: Option<Record> = {
		let query_bind_1 = grant;
		sqlx::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("semantic_remote_operations"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(grant_id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.order_by(Alias::new("created_at"), Order::Desc)
				.order_by(Alias::new("id"), Order::Desc)
				.limit(1)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&store.pool)
		.await?
	};
	if let Some(record) = record {
		result.state = record.state.to_lowercase();
		result.operation_id = Some(record.id);
		result.retry_count = record.failures.min(5);
		result.retry_at = record.next_attempt;
		result.reason = result.reason.or_else(|| {
			record
				.error
				.and_then(|s| serde_json::from_value(serde_json::json!(s)).ok())
		});
		if result.reason.is_none()
			&& let Some(value) = record.receipt
		{
			let receipt: Receipt = serde_json::from_value(value)?;
			result.result_count = Some(receipt.result.matches.len());
			result.truncated = receipt.result.truncated || receipt.query_truncated;
			result.retrieved_at = Some(receipt.retrieved_at);
			result.state = if result.truncated {
				"truncated"
			} else if receipt.result.matches.is_empty() {
				"empty"
			} else {
				"ready"
			}
			.into();
		}
	}
	if let Some(reason) = result.reason {
		result.state = if reason == Failure::Invalidated {
			"invalidated"
		} else if result.retry_at.is_some() {
			"waiting"
		} else {
			"paused"
		}
		.into();
	}
	Ok(result)
}

use reinhardt::query::QueryStatementBuilder as _;

use reinhardt::query::ColumnRef::Asterisk;

use reinhardt::query::SimpleExpr;
