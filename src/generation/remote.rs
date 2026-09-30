//! Origin-owned usage for providers serving a scoped execution on another node.
//! An owner derives its lineage from current authority, never a caller's list.
pub(crate) mod dispatch;
pub(crate) mod protocol;
use super::{Request, policy};
use crate::{
	Error, Result,
	authorization::{access::Access, catalog},
	registry::{AgentConfig, digest},
	semantic::remote::{Failure, Provider},
	store::Store,
};
use chrono::Utc;
use sea_orm::sea_query::{
	Alias, Asterisk, Expr, LockType, OnConflict, Order, PostgresQueryBuilder, Query,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = GenerationRemoteAllowance)]
pub struct Allowance {
	pub provider: Provider,
	pub calls_per_agent: i64,
	pub call_budget: i64,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = GenerationRemoteApprovals)]
pub struct Approvals {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub embedding: Option<Allowance>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub compaction: Option<Allowance>,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub inference: Vec<Provider>,
}
fn valid_digest(s: &str) -> bool {
	s.strip_prefix("sha256:")
		.is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|c| c.is_ascii_hexdigit()))
}
impl Approvals {
	pub(crate) fn validate(&self) -> Result<()> {
		if self.inference.len() > 32 {
			return Err(Error::Invalid("too many remote inference approvals".into()));
		}
		for allowance in self.embedding.iter().chain(self.compaction.iter()) {
			if !(1..=1_000_000).contains(&allowance.call_budget)
				|| !(1..=allowance.call_budget).contains(&allowance.calls_per_agent)
			{
				return Err(Error::Invalid(
					"invalid remote provider call allowance".into(),
				));
			}
		}
		for provider in self
			.inference
			.iter()
			.chain(self.embedding.iter().map(|a| &a.provider))
			.chain(self.compaction.iter().map(|a| &a.provider))
		{
			crate::config::validate_node_id(&provider.node_id)?;
			crate::domain::nonempty(&provider.entry.id, "remote provider")?;
			crate::domain::nonempty(&provider.entry.version, "remote provider version")?;
			if [&provider.digest, &provider.configuration_digest]
				.iter()
				.any(|s| !valid_digest(s))
			{
				return Err(Error::Invalid(
					"remote provider approval requires exact definition and configuration digests"
						.into(),
				));
			}
		}
		Ok(())
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Purpose {
	Embedding,
	Inference,
	Compaction,
}
impl Purpose {
	pub(crate) fn name(self) -> &'static str {
		match self {
			Self::Embedding => "embedding",
			Self::Inference => "inference",
			Self::Compaction => "compaction",
		}
	}
	fn action(self) -> &'static str {
		match self {
			Self::Embedding => "embedding.invoke",
			Self::Inference => "model.infer",
			Self::Compaction => "compaction.invoke",
		}
	}
	fn counter(self) -> Option<(&'static str, &'static str)> {
		match self {
			Self::Embedding => Some(("embedding_calls", "embedding_call_limit")),
			Self::Compaction => Some(("compaction_calls", "compaction_call_limit")),
			Self::Inference => None,
		}
	}
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = GenerationAncestor)]
pub struct Ancestor {
	pub node_id: String,
	pub tenant: String,
	pub request_id: Uuid,
	pub policy_id: String,
	pub policy_revision: i64,
	pub depth: i32,
	pub expires_at: chrono::DateTime<Utc>,
}
impl Ancestor {
	fn new(node: &str, job: &Request) -> Self {
		Self {
			node_id: node.into(),
			tenant: job.tenant.clone(),
			request_id: job.id,
			policy_id: job.policy_id.clone(),
			policy_revision: job.policy_revision,
			depth: job.depth,
			expires_at: job.expires_at,
		}
	}
}

async fn jobs(access: &mut Access, node: &str) -> Result<Vec<Request>> {
	Ok(sqlx::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("generation_requests"))
			.and_where(Expr::cust(
				"tenant=$1 AND ($2 || '/agents/' || agent_id || '@' || agent_version)=ANY($3)",
			))
			.order_by(Alias::new("id"), Order::Asc)
			.to_string(PostgresQueryBuilder),
	)
	.bind(&access.identity.tenant)
	.bind(node)
	.bind(&access.subjects)
	.fetch_all(&mut **access.tx)
	.await?)
}
pub(crate) async fn lineage(access: &mut Access, node: &str) -> Result<Vec<Ancestor>> {
	let jobs = jobs(access, node).await?;
	for job in &jobs {
		let current = policy::load(&mut access.tx, &job.tenant, &job.policy_id, false).await?;
		if (job.status != "ACTIVE"
			&& !(job.status == "QUEUED" && job.prepared && !job.home_node.is_empty()))
			|| job.expires_at <= Utc::now()
			|| !current.spec.enabled
			|| job.quota_released
		{
			return Err(Error::RemoteSemantic(Failure::Authority));
		}
	}
	Ok(jobs.iter().map(|j| Ancestor::new(node, j)).collect())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Usage {
	pub operation_id: Uuid,
	pub attempt_id: Uuid,
	pub dispatcher_node: String,
	pub grant_id: Uuid,
	pub admission_id: Uuid,
	pub purpose: Purpose,
	pub provider: Provider,
	pub input_digest: String,
	pub reserved_tokens: i64,
}
impl Usage {
	pub(crate) fn digest(&self) -> Result<String> {
		Ok(digest(&serde_json::to_value(self)?))
	}
	pub(crate) fn validate(&self) -> Result<()> {
		if [
			self.operation_id,
			self.attempt_id,
			self.grant_id,
			self.admission_id,
		]
		.iter()
		.any(Uuid::is_nil)
			|| !(1..=1_000_000_000_000_i64).contains(&self.reserved_tokens)
			|| !valid_digest(&self.input_digest)
			|| self.dispatcher_node != self.provider.node_id
		{
			return Err(Error::RemoteSemantic(Failure::ProviderContract));
		}
		crate::config::validate_node_id(&self.dispatcher_node)
	}
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Reserved {
	pub owner: Ancestor,
	pub attempt_id: Uuid,
	pub digest: String,
}

async fn approved(access: &mut Access, node: &str, job: &Request, usage: &Usage) -> Result<()> {
	let current = policy::load(&mut access.tx, &job.tenant, &job.policy_id, false).await?;
	if job.status != "ACTIVE"
		|| job.expires_at <= Utc::now()
		|| job.quota_released
		|| !current.spec.enabled
	{
		return Err(Error::RemoteSemantic(Failure::Authority));
	}
	let value: Value = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("spec"))
			.from(Alias::new("generation_policy_history"))
			.and_where(Expr::cust("tenant=$1 AND policy_id=$2 AND revision=$3"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(&job.tenant)
	.bind(&job.policy_id)
	.bind(job.policy_revision)
	.fetch_one(&mut **access.tx)
	.await?;
	let spec: policy::Spec = serde_json::from_value(value)?;
	if usage.provider.node_id == node {
		let expected = match usage.purpose {
			Purpose::Embedding => spec.embedding.map(|v| v.provider),
			Purpose::Compaction => spec.compaction.map(|v| v.provider),
			Purpose::Inference => {
				Some(serde_json::from_value::<AgentConfig>(spec.template.config)?.model)
			}
		}
		.ok_or(Error::RemoteSemantic(Failure::Allowance))?;
		if expected != usage.provider.entry {
			return Err(Error::RemoteSemantic(Failure::Allowance));
		}
		let entry = catalog::entry(access, &expected, usage.purpose.action()).await?;
		access
			.require(&catalog::resource(access, &entry), "registry.read")
			.await?;
		if digest(&serde_json::to_value(&entry)?) != usage.provider.digest
			|| digest(&entry.config) != usage.provider.configuration_digest
		{
			return Err(Error::RemoteSemantic(Failure::Configuration));
		}
	} else {
		let approvals = spec
			.remote
			.ok_or(Error::RemoteSemantic(Failure::Allowance))?;
		let permitted = match usage.purpose {
			Purpose::Embedding => approvals
				.embedding
				.as_ref()
				.is_some_and(|v| v.provider == usage.provider),
			Purpose::Compaction => approvals
				.compaction
				.as_ref()
				.is_some_and(|v| v.provider == usage.provider),
			Purpose::Inference => approvals.inference.contains(&usage.provider),
		};
		if !permitted {
			return Err(Error::RemoteSemantic(Failure::Allowance));
		}
		// The exact descriptor grants no local secret or catalog alias resolution.
		let resource=access.resource("registry",format!("{}/registry/{}@{}",usage.provider.node_id,usage.provider.entry.id,usage.provider.entry.version),json!({"remote_node":usage.provider.node_id,"definition_digest":usage.provider.digest,"purpose":usage.purpose.name()}));
		access.require(&resource, "registry.read").await?;
		access.require(&resource, usage.purpose.action()).await?;
	}
	Ok(())
}

/// Validate under the caller's current authority lease. Commit all local ancestor
/// debits together, separately from that lease, before returning any receipt.
pub(crate) async fn reserve(
	access: &mut Access,
	store: &Store,
	usage: &Usage,
) -> Result<Vec<Reserved>> {
	usage.validate()?;
	let jobs = jobs(access, &store.node_id).await?;
	for job in &jobs {
		approved(access, &store.node_id, job, usage).await?;
	}
	let digest = usage.digest()?;
	let mut tx = store.pool.begin().await?;
	if lock_attempt(&mut tx, usage.attempt_id, &digest)
		.await?
		.is_some()
	{
		return Err(Error::Conflict("provider attempt already finalized".into()));
	}
	let mut reservations = vec![];
	for job in jobs {
		// One sorted budget row lock serializes local, remote and duplicate RPCs.
		let _: Uuid = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("request_id"))
				.from(Alias::new("generation_budgets"))
				.and_where(Expr::cust("request_id=$1"))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.bind(job.id)
		.fetch_one(&mut *tx)
		.await?;
		let existing: Option<(String, String)> = sqlx::query_as(
			&Query::select()
				.columns(["digest", "state"].map(Alias::new))
				.from(Alias::new("generation_remote_usage"))
				.and_where(Expr::cust("request_id=$1 AND attempt_id=$2"))
				.to_string(PostgresQueryBuilder),
		)
		.bind(job.id)
		.bind(usage.attempt_id)
		.fetch_optional(&mut *tx)
		.await?;
		if let Some((previous, state)) = existing {
			if previous != digest || state != "RESERVED" {
				return Err(Error::Conflict(
					"provider attempt already has a different or final reservation".into(),
				));
			}
		} else {
			let mut q = Query::update();
			q.table(Alias::new("generation_budgets"))
				.value(Alias::new("used_tokens"), Expr::cust("used_tokens+$2"))
				.and_where(Expr::cust(
					"request_id=$1 AND token_limit-used_tokens >= $2",
				));
			if let Some((counter, limit)) = usage.purpose.counter() {
				q.value(Alias::new(counter), Expr::cust(format!("{counter}+1")))
					.and_where(Expr::cust(format!("{counter} < {limit}")));
			}
			if sqlx::query(&q.to_string(PostgresQueryBuilder))
				.bind(job.id)
				.bind(usage.reserved_tokens)
				.execute(&mut *tx)
				.await?
				.rows_affected()
				!= 1
			{
				return Err(Error::RemoteSemantic(Failure::Allowance));
			}
			sqlx::query(
				&Query::insert()
					.into_table(Alias::new("generation_remote_usage"))
					.columns(
						[
							"request_id",
							"attempt_id",
							"operation_id",
							"dispatcher_node",
							"grant_id",
							"admission_id",
							"purpose",
							"digest",
							"reserved_tokens",
						]
						.map(Alias::new),
					)
					.values_panic(
						["$1", "$2", "$3", "$4", "$5", "$6", "$7", "$8", "$9"].map(Expr::cust),
					)
					.to_string(PostgresQueryBuilder),
			)
			.bind(job.id)
			.bind(usage.attempt_id)
			.bind(usage.operation_id)
			.bind(&usage.dispatcher_node)
			.bind(usage.grant_id)
			.bind(usage.admission_id)
			.bind(usage.purpose.name())
			.bind(&digest)
			.bind(usage.reserved_tokens)
			.execute(&mut *tx)
			.await?;
		}
		reservations.push(Reserved {
			owner: Ancestor::new(&store.node_id, &job),
			attempt_id: usage.attempt_id,
			digest: digest.clone(),
		});
	}
	tx.commit().await?;
	Ok(reservations)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Finalization {
	Settled { reported: Option<i64> },
	Aborted {},
}

/// The attempt row serializes reservation insertion and finalization even
/// when finalization arrives before any usage row exists.
async fn lock_attempt(
	tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
	attempt: Uuid,
	digest: &str,
) -> Result<Option<Value>> {
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("generation_remote_finalizations"))
			.columns(["attempt_id", "digest"].map(Alias::new))
			.values_panic(["$1", "$2"].map(Expr::cust))
			.on_conflict(
				OnConflict::column(Alias::new("attempt_id"))
					.do_nothing()
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.bind(attempt)
	.bind(digest)
	.execute(&mut **tx)
	.await?;
	let (saved_digest, result): (String, Option<Value>) = sqlx::query_as(
		&Query::select()
			.columns(["digest", "result"].map(Alias::new))
			.from(Alias::new("generation_remote_finalizations"))
			.and_where(Expr::cust("attempt_id=$1"))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.bind(attempt)
	.fetch_one(&mut **tx)
	.await?;
	if saved_digest != digest {
		return Err(Error::Conflict(
			"provider attempt has a different reservation".into(),
		));
	}
	Ok(result)
}
/// Called only by the bound dispatcher's authenticated finalization path. A
/// pre-dispatch abort must already be durable before the caller may release.
/// Unknown or oversized usage retains the entire debit; settlement is a CAS.
pub(crate) async fn finalize(store: &Store, usage: &Usage, result: &Finalization) -> Result<()> {
	usage.validate()?;
	let digest = usage.digest()?;
	let (state, reported, refund) = match result {
		Finalization::Aborted {} => ("RELEASED", None, usage.reserved_tokens),
		Finalization::Settled { reported } => (
			"SETTLED",
			*reported,
			reported
				.filter(|n| *n > 0 && *n <= usage.reserved_tokens)
				.map_or(0, |n| usage.reserved_tokens - n),
		),
	};
	let mut tx = store.pool.begin().await?;
	if let Some(previous) = lock_attempt(&mut tx, usage.attempt_id, &digest).await?
		&& previous != json!(result)
	{
		return Err(Error::Conflict(
			"provider attempt already finalized differently".into(),
		));
	}
	let rows: Vec<(Uuid, String, Option<i64>)> = sqlx::query_as(
		&Query::select()
			.columns(["request_id", "state", "reported_tokens"].map(Alias::new))
			.from(Alias::new("generation_remote_usage"))
			.and_where(Expr::cust("attempt_id=$1 AND digest=$2"))
			.order_by(Alias::new("request_id"), Order::Asc)
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.bind(usage.attempt_id)
	.bind(&digest)
	.fetch_all(&mut *tx)
	.await?;
	for (id, old, old_reported) in rows {
		if old != "RESERVED" {
			if old != state || old_reported != reported {
				return Err(Error::Conflict(
					"provider attempt already finalized differently".into(),
				));
			}
			continue;
		}
		let mut q = Query::update();
		q.table(Alias::new("generation_budgets"))
			.value(Alias::new("used_tokens"), Expr::cust("used_tokens-$2"))
			.and_where(Expr::cust("request_id=$1 AND used_tokens >= $2"));
		if state == "RELEASED"
			&& let Some((counter, _)) = usage.purpose.counter()
		{
			q.value(Alias::new(counter), Expr::cust(format!("{counter}-1")))
				.and_where(Expr::cust(format!("{counter}>0")));
		}
		if sqlx::query(&q.to_string(PostgresQueryBuilder))
			.bind(id)
			.bind(refund)
			.execute(&mut *tx)
			.await?
			.rows_affected()
			!= 1
		{
			return Err(Error::Conflict(
				"provider refund exceeds committed reservation".into(),
			));
		}
		// Terminal lifecycle may already have released the then-unused quota.
		let job: Request = sqlx::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("generation_requests"))
				.and_where(Expr::cust("id=$1"))
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.fetch_one(&mut *tx)
		.await?;
		if job.quota_released {
			let mut q = Query::update();
			q.table(Alias::new("generation_policies"))
				.value(
					Alias::new("allocated_tokens"),
					Expr::cust("allocated_tokens-$3"),
				)
				.and_where(Expr::cust("tenant=$1 AND id=$2 AND allocated_tokens >= $3"));
			if state == "RELEASED"
				&& let Some((_, limit)) = usage.purpose.counter()
			{
				let allocation = if limit == "embedding_call_limit" {
					"allocated_embedding_calls"
				} else {
					"allocated_compaction_calls"
				};
				q.value(
					Alias::new(allocation),
					Expr::cust(format!("{allocation}-1")),
				)
				.and_where(Expr::cust(format!("{allocation}>0")));
			}
			if sqlx::query(&q.to_string(PostgresQueryBuilder))
				.bind(&job.tenant)
				.bind(&job.policy_id)
				.bind(refund)
				.execute(&mut *tx)
				.await?
				.rows_affected()
				!= 1
			{
				return Err(Error::Conflict(
					"provider refund exceeds policy allocation".into(),
				));
			}
		}
		sqlx::query(
			&Query::update()
				.table(Alias::new("generation_remote_usage"))
				.value(Alias::new("state"), Expr::cust("$3"))
				.value(Alias::new("reported_tokens"), Expr::cust("$4"))
				.and_where(Expr::cust(
					"request_id=$1 AND attempt_id=$2 AND state='RESERVED'",
				))
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.bind(usage.attempt_id)
		.bind(state)
		.bind(reported)
		.execute(&mut *tx)
		.await?;
	}
	sqlx::query(
		&Query::update()
			.table(Alias::new("generation_remote_finalizations"))
			.value(Alias::new("result"), Expr::cust("$2"))
			.and_where(Expr::cust("attempt_id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(usage.attempt_id)
	.bind(json!(result))
	.execute(&mut *tx)
	.await?;
	tx.commit().await?;
	if reported.is_some_and(|n| n > usage.reserved_tokens) {
		return Err(Error::RemoteSemantic(Failure::ProviderContract));
	}
	Ok(())
}

pub(crate) fn verify_receipts(
	expected: &[Ancestor],
	receipts: &[Reserved],
	usage: &Usage,
) -> Result<()> {
	let digest = usage.digest()?;
	if expected.len() != receipts.len()
		|| expected
			.iter()
			.zip(receipts)
			.any(|(a, r)| a != &r.owner || r.attempt_id != usage.attempt_id || r.digest != digest)
	{
		return Err(Error::RemoteSemantic(Failure::ProviderContract));
	}
	Ok(())
}
