//! Model Call ownership, durable dispatch and prompt-free usage.
use crate::{Error, Result, database::native, store::Store};
use aidash_application::tenancy::TenancyScope;
use aidash_domain::{
	model::ModelConfig,
	model_call::{Dispatch, Outcome, Purpose},
	provider::{
		ModelRequest,
		usage::{ReportedUsage, UsageDispatch, UsageOutcome, UsagePurpose},
	},
	registry::EntityRef,
};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use uuid::Uuid;

use super::super::models::InferenceUsage;

pub(crate) struct Attribution {
	pub tenant: String,
	pub node: String,
	pub run: Option<Uuid>,
	pub stint: Option<Uuid>,
}

impl Attribution {
	pub(crate) fn node(tenant: String, node: String) -> Self {
		Self {
			tenant,
			node,
			run: None,
			stint: None,
		}
	}

	async fn for_run(tx: &mut dyn TransactionExecutor, run: Uuid) -> Result<Self> {
		let row = native::query(
			&Query::select()
				.columns(["tenant", "node"].map(Alias::new))
				.from(Alias::new("task_executions"))
				.and_where(Expr::col("id").eq(Expr::value(run)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&mut *tx)
		.await?;
		let tenant: String = row.try_get("tenant")?;
		let node: String = row.try_get("node")?;
		let stint: Option<Uuid> = native::query_scalar(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("stints"))
				.and_where(Expr::col("tenant").eq(Expr::value(tenant.clone())))
				.and_where(Expr::col("run_id").eq(Expr::value(run)))
				.and_where(Expr::col("ended_at").is_null())
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut *tx)
		.await?;
		Ok(Self {
			tenant,
			node,
			run: stint.map(|_| run),
			stint,
		})
	}

	pub(crate) async fn awake(tx: &mut dyn TransactionExecutor, run: Uuid) -> Result<Self> {
		let owner = Self::for_run(tx, run).await?;
		if owner.stint.is_none() {
			return Err(Error::Conflict("model call requires an open Stint".into()));
		}
		Ok(owner)
	}
}

pub(crate) async fn start(
	tx: &mut dyn TransactionExecutor,
	id: Uuid,
	owner: &Attribution,
	model: &EntityRef,
	call: &Dispatch,
) -> Result<()> {
	let mut insert = Query::insert();
	insert.into_table(Alias::new("model_calls")).columns(
		[
			"id",
			"tenant",
			"node",
			"run_id",
			"stint_id",
			"purpose",
			"model_id",
			"model_version",
			"request_digest",
			"retry_key",
			"started_at",
		]
		.map(Alias::new),
	);
	insert.from_subquery(
		Query::select()
			.expr(Expr::value(id))
			.expr(Expr::value(owner.tenant.clone()))
			.expr(Expr::value(owner.node.clone()))
			.expr(Expr::value(owner.run))
			.expr(Expr::value(owner.stint))
			.expr(Expr::value(call.purpose.as_str()))
			.expr(Expr::value(model.id.clone()))
			.expr(Expr::value(model.version.clone()))
			.expr(Expr::value(call.request_digest.clone()))
			.expr(Expr::value(call.retry_key.clone()))
			.expr(Expr::current_timestamp())
			.to_owned(),
	);
	native::query(&insert.to_string(PostgresQueryBuilder))
		.execute(&mut *tx)
		.await?;
	Ok(())
}

/// Find an earlier attempt of this same Run request. The worker lease
/// serializes competing dispatches even if the Runtimes span Stints.
pub(crate) async fn start_run_call(
	tx: &mut dyn TransactionExecutor,
	owner: &Attribution,
	attempt: Uuid,
	model: &EntityRef,
	dispatch: &Dispatch,
) -> Result<Uuid> {
	let mut dispatch = dispatch.clone();
	if dispatch.purpose == Purpose::CompactionSummary && dispatch.retry_key.is_none() {
		dispatch.retry_key = Some(dispatch.request_digest.clone());
	}
	if let Some(key) = &dispatch.retry_key {
		let existing: Option<Uuid> = native::query_scalar(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("model_calls"))
				.and_where(Expr::col("tenant").eq(Expr::value(owner.tenant.clone())))
				.and_where(Expr::col("run_id").eq(Expr::value(owner.run)))
				.and_where(Expr::col("purpose").eq(Expr::value(dispatch.purpose.as_str())))
				.and_where(Expr::col("retry_key").eq(Expr::value(key.clone())))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut *tx)
		.await?;
		if let Some(id) = existing {
			reopen_call(tx, id).await?;
			return Ok(id);
		}
	}
	start(tx, attempt, owner, model, &dispatch).await?;
	Ok(attempt)
}

pub(crate) async fn finish(
	tx: &mut dyn TransactionExecutor,
	id: Uuid,
	outcome: Outcome,
) -> Result<()> {
	native::query(
		&Query::update()
			.table(Alias::new("model_calls"))
			.value_expr(Alias::new("outcome"), Expr::value(outcome.as_str()))
			.value_expr(Alias::new("ended_at"), Expr::current_timestamp())
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.and_where(Expr::col("ended_at").is_null())
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	Ok(())
}

pub(crate) fn reported(usage: &UsageOutcome) -> Option<&ReportedUsage> {
	match usage {
		UsageOutcome::Completed(reported) => Some(reported),
		UsageOutcome::Unknown | UsageOutcome::Rejected { .. } => None,
	}
}

/// Start a call whose admission and Budget settlement are owned by its caller.
pub(crate) struct Auxiliary<'a> {
	pub model: &'a EntityRef,
	pub config: &'a ModelConfig,
	pub request: &'a ModelRequest,
	pub purpose: Purpose,
	pub replay_key: Option<&'a str>,
	pub timeout: std::time::Duration,
}

pub(crate) struct Attempt {
	pub id: Uuid,
	pub call: Uuid,
}

pub(crate) async fn start_auxiliary(
	tx: &mut dyn TransactionExecutor,
	owner: &Attribution,
	call: Auxiliary<'_>,
	id: Uuid,
	prior_attempts: &[Uuid],
) -> Result<Attempt> {
	let Auxiliary {
		model,
		config,
		request,
		purpose,
		replay_key,
		timeout,
	} = call;
	let mut dispatch = Dispatch::new(purpose, request)?;
	dispatch.retry_key = replay_key.map(str::to_owned);
	let prior = if prior_attempts.is_empty() {
		None
	} else {
		reopen(tx, owner, &dispatch, prior_attempts).await?
	};
	let (call, run) = if let Some(prior) = prior {
		prior
	} else {
		start(tx, id, owner, model, &dispatch).await?;
		(id, owner.run)
	};
	let usage = UsageDispatch {
		purpose: UsagePurpose::Auxiliary,
		model_call: None,
		attempt: id,
		lease: id,
		response_epoch: 0,
		model_id: model.id.clone(),
		model_version: model.version.clone(),
		projection_version: request.context.projection_version().number(),
		harness_contract: aidash_domain::harness_contract::HarnessContract::default().number(),
		upstream_model: config.model_id.clone(),
		estimate: request.request_estimate(),
		requested_route: config
			.provider_route
			.as_ref()
			.map(|route| route.tag.clone()),
		shadow_estimates: vec![],
		calibration_sample: false,
		cache_identity: request.cache_identity(),
	};
	InferenceUsage::insert_dispatch(tx, owner.tenant.clone(), run, Some(call), &usage).await?;
	let deadline = chrono::Utc::now()
		+ chrono::TimeDelta::from_std(timeout)
			.map_err(|_| Error::Invalid("model call timeout is out of range".into()))?
		+ chrono::TimeDelta::minutes(1);
	native::query(
		&Query::update()
			.table(Alias::new("inference_usage"))
			.value_expr(Alias::new("recovery_after"), Expr::value(deadline))
			.and_where(Expr::col("attempt_id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;

	Ok(Attempt { id, call })
}

/// Resume the same logical call while allocating a new physical attempt.
async fn reopen(
	tx: &mut dyn TransactionExecutor,
	owner: &Attribution,
	dispatch: &Dispatch,
	prior_attempts: &[Uuid],
) -> Result<Option<(Uuid, Option<Uuid>)>> {
	let calls = Query::select()
		.column(Alias::new("model_call_id"))
		.from(Alias::new("inference_usage"))
		.and_where(Expr::col("tenant").eq(Expr::value(owner.tenant.clone())))
		.and_where(Expr::col("attempt_id").is_in(prior_attempts.iter().copied().map(Expr::value)))
		.to_owned();
	let mut query = Query::select();
	query
		.columns(["id", "run_id", "retry_key"].map(Alias::new))
		.from(Alias::new("model_calls"))
		.and_where(Expr::col("tenant").eq(Expr::value(owner.tenant.clone())))
		.and_where(Expr::col("id").in_subquery(calls))
		.and_where(Expr::col("node").eq(Expr::value(owner.node.clone())))
		.order_by("started_at", reinhardt::query::Order::Desc)
		.order_by("id", reinhardt::query::Order::Desc)
		.limit(1);
	if let Some(run) = owner.run {
		query.and_where(Expr::col("run_id").eq(Expr::value(run)));
	} else {
		query.and_where(Expr::col("run_id").is_null());
	}
	let row = native::query(&query.to_string(PostgresQueryBuilder))
		.fetch_optional(&mut *tx)
		.await?;
	let Some(row) = row else {
		return Ok(None);
	};
	let key: Option<String> = row.try_get("retry_key")?;
	if key.is_some() && key != dispatch.retry_key {
		return Err(Error::Conflict(
			"retried model call changed its request".into(),
		));
	}
	let call: Uuid = row.try_get("id")?;
	for attempt in prior_attempts {
		InferenceUsage::abandon_auxiliary_attempt(tx, *attempt).await?;
	}
	reopen_call(tx, call).await?;
	Ok(Some((call, row.try_get("run_id")?)))
}

async fn reopen_call(tx: &mut dyn TransactionExecutor, call: Uuid) -> Result<()> {
	native::query(
		&Query::update()
			.table(Alias::new("model_calls"))
			.value_expr(Alias::new("outcome"), Expr::value(Option::<String>::None))
			.value_expr(
				Alias::new("ended_at"),
				Expr::value(Option::<chrono::DateTime<chrono::Utc>>::None),
			)
			.and_where(Expr::col("id").eq(Expr::value(call)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	Ok(())
}

pub(crate) async fn finish_auxiliary(
	tx: &mut dyn TransactionExecutor,
	attempt: Attempt,
	result: &aidash_application::Result<aidash_domain::provider::ModelResponse>,
) -> Result<bool> {
	let usage = aidash_application::model_calls::usage(result);
	let outcome = if InferenceUsage::write_completion(tx, attempt.id, &usage).await? {
		Some(aidash_application::model_calls::outcome(result))
	} else if matches!(usage, UsageOutcome::Completed(_))
		&& InferenceUsage::write_late_completion(tx, attempt.id, &usage).await?
	{
		None
	} else {
		return Ok(false);
	};
	settle(tx, attempt.call, outcome, reported(&usage)).await?;
	Ok(true)
}

/// Accumulate each persisted attempt's reported use. A late result supplies
/// provider facts without changing the logical call's current outcome.
pub(crate) async fn settle(
	tx: &mut dyn TransactionExecutor,
	call: Uuid,
	outcome: Option<Outcome>,
	reported: Option<&ReportedUsage>,
) -> Result<()> {
	use aidash_domain::provider::usage::ProviderCost;
	use reinhardt::query::LockType;
	let previous: Option<serde_json::Value> = native::query_scalar(
		&Query::select()
			.column(Alias::new("usage"))
			.from(Alias::new("model_calls"))
			.and_where(Expr::col("id").eq(Expr::value(call)))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut *tx)
	.await?;
	let mut total: Option<ReportedUsage> = previous.map(serde_json::from_value).transpose()?;
	if let Some(reported) = reported {
		let combined = total.get_or_insert_with(ReportedUsage::default);
		let sum = |left: Option<u64>, right: Option<u64>| match (left, right) {
			(Some(left), Some(right)) => left.checked_add(right),
			(left, right) => left.or(right),
		};
		combined.input_tokens = sum(combined.input_tokens, reported.input_tokens);
		combined.output_tokens = sum(combined.output_tokens, reported.output_tokens);
		combined.cache_read_tokens = sum(combined.cache_read_tokens, reported.cache_read_tokens);
		combined.cache_write_tokens = sum(combined.cache_write_tokens, reported.cache_write_tokens);
		combined.reasoning_tokens = sum(combined.reasoning_tokens, reported.reasoning_tokens);
		combined.cost = match (combined.cost, reported.cost) {
			(Some(previous), Some(current)) => previous
				.nanocredits
				.checked_add(current.nanocredits)
				.map(|nanocredits| ProviderCost {
					nanocredits,
					upstream_nanocredits: match (
						previous.upstream_nanocredits,
						current.upstream_nanocredits,
					) {
						(Some(left), Some(right)) => left.checked_add(right),
						(left, right) => left.or(right),
					},
				}),
			(previous, current) => previous.or(current),
		};
		for (tool, count) in &reported.server_tool_use {
			combined
				.server_tool_use
				.entry(tool.clone())
				.and_modify(|total| *total = total.saturating_add(*count))
				.or_insert(*count);
		}
		combined.provider_identity = reported.provider_identity.clone();
	}
	let usage = total.map(serde_json::to_value).transpose()?;
	let mut change = Query::update();
	change
		.table(Alias::new("model_calls"))
		.value_expr(Alias::new("usage"), Expr::value(usage))
		.and_where(Expr::col("id").eq(Expr::value(call)));
	if let Some(outcome) = outcome {
		change
			.value_expr(Alias::new("outcome"), Expr::value(outcome.as_str()))
			.value_expr(Alias::new("ended_at"), Expr::current_timestamp());
	}
	native::query(&change.to_string(PostgresQueryBuilder))
		.execute(&mut *tx)
		.await?;
	Ok(())
}

const WORKBENCH_MODEL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// Workbench tests carry a Tenant but no Node in their admission contract.
/// Use that Tenant's default Node for Model Call attribution and Budget.
pub(crate) struct WorkbenchProvider {
	pub store: Store,
	pub tenant: String,
	pub model: EntityRef,
	pub config: ModelConfig,
	pub provider: std::sync::Arc<dyn aidash_application::ports::ModelProvider>,
}

#[async_trait::async_trait]
impl aidash_application::ports::ModelProvider for WorkbenchProvider {
	async fn infer(
		&self,
		request: ModelRequest,
		progress: &dyn aidash_application::ports::InferenceProgressSink,
	) -> aidash_application::Result<aidash_domain::provider::ModelResponse> {
		use crate::apps::identity::repositories::budgets;
		use aidash_application::model_calls;
		use chrono::Utc;

		let scope = TenancyScope::tenant(&self.tenant)?;
		let (attempt, charge) = self
			.store
			.control_pool
			.atomic(&scope, async |tx| {
				let node = crate::apps::identity::repositories::nodes::default_of(tx, &self.tenant)
					.await?;
				let owner = Attribution::node(self.tenant.clone(), node.name.to_string());
				let charge = budgets::Charge {
					tenant: owner.tenant.clone(),
					node: owner.node.clone(),
					root_subject: String::new(),
					duty: Uuid::nil(),
				};
				let (gate, _) = budgets::gate_for(tx, &charge, Utc::now()).await?;
				budgets::admit(gate)?;
				let attempt = start_auxiliary(
					tx,
					&owner,
					Auxiliary {
						model: &self.model,
						config: &self.config,
						request: &request,
						purpose: Purpose::WorkbenchTest,
						replay_key: None,
						timeout: WORKBENCH_MODEL_TIMEOUT,
					},
					Uuid::new_v4(),
					&[],
				)
				.await?;
				Ok((attempt, charge))
			})
			.await?;
		let result = tokio::time::timeout(
			WORKBENCH_MODEL_TIMEOUT,
			self.provider.infer(request, progress),
		)
		.await
		.unwrap_or_else(|_| {
			Err(aidash_application::Error::External(
				"Workbench model request timed out".into(),
			))
		});
		let usage = model_calls::usage(&result);
		self.store
			.control_pool
			.atomic(&scope, async |tx| {
				if finish_auxiliary(tx, attempt, &result).await?
					&& !matches!(usage, UsageOutcome::Rejected { .. })
				{
					budgets::record_charge(
						tx,
						&charge,
						reported(&usage)
							.and_then(|usage| usage.cost)
							.map(|cost| cost.nanocredits),
						Utc::now(),
					)
					.await?;
				}
				Ok(())
			})
			.await?;
		result
	}
}

/// Recover auxiliary dispatches that outlived their provider deadline. Costs
/// are posted only when a provider report arrives, including a late report.
pub(crate) async fn recover_auxiliary(store: &Store) -> Result<usize> {
	use aidash_application::tenancy::DeploymentScopeReason;
	use reinhardt::query::{LockType, Order};
	let scope = TenancyScope::Deployment(DeploymentScopeReason::Retention);
	store
		.control_pool
		.atomic(&scope, async |tx| {
			let now = chrono::Utc::now();
			let expired = Expr::col("recovery_after")
				.lt(Expr::value(now))
				.or(Expr::col("recovery_after").is_null().and(
					Expr::col("created_at").lt(Expr::value(now - chrono::TimeDelta::minutes(15))),
				));
			let rows = native::query(
				&Query::select()
					.columns(["attempt_id", "model_call_id"].map(Alias::new))
					.from(Alias::new("inference_usage"))
					.and_where(
						Expr::col("purpose").eq(Expr::value(UsagePurpose::Auxiliary.as_str())),
					)
					.and_where(Expr::col("outcome").eq(Expr::value("dispatched")))
					.and_where(Expr::col("model_call_id").is_not_null())
					.and_where(expired)
					.order_by("recovery_after", Order::Asc)
					.order_by("attempt_id", Order::Asc)
					.limit(100)
					.lock(LockType::Update)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(&mut *tx)
			.await?;
			for row in &rows {
				let id = row.try_get("attempt_id")?;
				let call = row.try_get("model_call_id")?;
				if InferenceUsage::abandon_auxiliary_attempt(tx, id).await? {
					finish(tx, call, Outcome::Failed).await?;
				}
			}
			Ok(rows.len())
		})
		.await
}

/// Read Stints and their Model Calls under the Run inspection disclosure lease.
pub(crate) async fn stints(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	run: Uuid,
	offset: u64,
) -> Result<Vec<aidash_domain::model_call::Stint>> {
	use aidash_domain::model_call::{ModelCall, Owner, Stint};
	use reinhardt::query::Order;
	let calls = native::query(
		&Query::select()
			.columns(
				[
					"id",
					"stint_id",
					"purpose",
					"model_id",
					"model_version",
					"request_digest",
					"outcome",
					"usage",
					"started_at",
					"ended_at",
				]
				.map(Alias::new),
			)
			.from(Alias::new("model_calls"))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("tenant").eq(Expr::value(tenant.to_owned())))
			.and_where(
				Expr::col("purpose")
					.is_in(Purpose::ALL.map(|purpose| Expr::value(purpose.as_str()))),
			)
			.order_by("started_at", Order::Asc)
			.order_by("id", Order::Asc)
			.limit(100)
			.offset(offset)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut *tx)
	.await?;
	let mut grouped = std::collections::BTreeMap::<Uuid, Vec<ModelCall>>::new();
	for row in calls {
		let stint_id = row.try_get("stint_id")?;
		let purpose: String = row.try_get("purpose")?;
		let outcome: Option<String> = row.try_get("outcome")?;
		let usage: Option<serde_json::Value> = row.try_get("usage")?;
		grouped.entry(stint_id).or_default().push(ModelCall {
			id: row.try_get("id")?,
			purpose: serde_json::from_value(purpose.into())?,
			owner: Owner::Stint {
				stint_id,
				run_id: run,
			},
			model: EntityRef {
				id: row.try_get("model_id")?,
				version: row.try_get("model_version")?,
			},
			request_digest: row.try_get("request_digest")?,
			outcome: outcome
				.map(|value| serde_json::from_value(value.into()))
				.transpose()?,
			usage: usage.map(serde_json::from_value).transpose()?,
			started_at: row.try_get("started_at")?,
			ended_at: row.try_get("ended_at")?,
		});
	}
	let columns = [
		"id",
		"node",
		"agent_id",
		"agent_version",
		"started_at",
		"ended_at",
		"end_reason",
	]
	.map(Alias::new);
	// A call can outnumber its owner's page, so include the owners of this call page too.
	let mut page = Query::select();
	page.column(Alias::new("id"))
		.from(Alias::new("stints"))
		.and_where(Expr::col("run_id").eq(Expr::value(run)))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant.to_owned())))
		.order_by("started_at", Order::Asc)
		.order_by("id", Order::Asc)
		.limit(100)
		.offset(offset);
	let rows = native::query(
		&Query::select()
			.columns(columns)
			.from(Alias::new("stints"))
			.and_where(Expr::col("tenant").eq(Expr::value(tenant.to_owned())))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(
				Expr::col("id")
					.in_subquery(page)
					.or(Expr::col("id").is_in(grouped.keys().copied().map(Expr::value))),
			)
			.order_by("started_at", Order::Asc)
			.order_by("id", Order::Asc)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut *tx)
	.await?;
	rows.into_iter()
		.map(|row| {
			let id = row.try_get("id")?;
			Ok(Stint {
				id,
				node: row.try_get("node")?,
				agent_id: row.try_get("agent_id")?,
				agent_version: row.try_get("agent_version")?,
				started_at: row.try_get("started_at")?,
				ended_at: row.try_get("ended_at")?,
				end_reason: row.try_get("end_reason")?,
				model_calls: grouped.remove(&id).unwrap_or_default(),
			})
		})
		.collect()
}

pub(crate) async fn tenant_usage(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
) -> Result<Vec<aidash_domain::model_call::UsageByPurpose>> {
	use aidash_domain::model_call::UsageByPurpose;
	use reinhardt::query::{Func, JoinType, Order, TableRef};
	let fields = ["input_tokens", "output_tokens", "cost_nanocredits"];
	let mut calls = Query::select();
	calls
		.column((Alias::new("m"), Alias::new("purpose")))
		.from_as(Alias::new("model_calls"), Alias::new("m"))
		.join(
			JoinType::LeftJoin,
			TableRef::table_alias(Alias::new("inference_usage"), Alias::new("u")),
			Expr::col((Alias::new("m"), Alias::new("id")))
				.equals((Alias::new("u"), Alias::new("model_call_id")))
				.and(
					Expr::col((Alias::new("m"), Alias::new("tenant")))
						.equals((Alias::new("u"), Alias::new("tenant"))),
				),
		)
		.and_where(
			Expr::col((Alias::new("m"), Alias::new("tenant"))).eq(Expr::value(tenant.to_owned())),
		)
		.and_where(Expr::col((Alias::new("m"), Alias::new("run_id"))).is_null())
		.and_where(
			Expr::col((Alias::new("m"), Alias::new("purpose")))
				.is_in(Purpose::ALL.map(|purpose| Expr::value(purpose.as_str()))),
		)
		.group_by_col((Alias::new("m"), Alias::new("id")))
		.group_by_col((Alias::new("m"), Alias::new("purpose")));
	for field in fields {
		calls.expr_as(
			Func::sum(Expr::col((Alias::new("u"), Alias::new(field))).into()),
			Alias::new(field),
		);
	}

	let mut query = Query::select();
	query
		.column(Alias::new("purpose"))
		.expr_as(
			Func::count(Expr::col(Alias::new("purpose")).into()),
			Alias::new("calls"),
		)
		.from_subquery(calls, Alias::new("calls_by_id"))
		.group_by_col(Alias::new("purpose"))
		.order_by("purpose", Order::Asc);
	for field in fields {
		query.expr_as(
			Func::sum(Expr::col(Alias::new(field)).into()).cast_as("int8"),
			Alias::new(field),
		);
	}
	native::query(&query.to_string(PostgresQueryBuilder))
		.fetch_all(&mut *tx)
		.await?
		.into_iter()
		.map(|row| {
			let purpose: String = row.try_get("purpose")?;
			Ok(UsageByPurpose {
				purpose: serde_json::from_value(purpose.into())?,
				calls: row.try_get("calls")?,
				input_tokens: row.try_get("input_tokens")?,
				output_tokens: row.try_get("output_tokens")?,
				cost_nanocredits: row.try_get("cost_nanocredits")?,
			})
		})
		.collect()
}
