//! Every charge and completion is fenced by the observer lease, never the Run lease.
use super::repository;
use crate::{Error, Result, database::native, store::Store};
use aidash_application::{
	decision::observation::{ObservationJournal, ObservationLease, observation_boundary},
	ports::decision::*,
	tenancy::TenancyScope,
};
use aidash_domain::{
	context::Context,
	decision::{observation::*, *},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::{
	db::{backends::TransactionExecutor, orm::execution::convert_values},
	query::{
		Alias, Expr, ExprTrait, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder,
		SimpleExpr,
	},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use uuid::Uuid;

pub(super) struct Journal {
	pub store: Store,
	pub frame: ObservationFrame,
	pub lease: ObservationLease,
}
impl Journal {
	fn receipts(&self, record: &DispatchRecord) -> BTreeMap<String, Uuid> {
		let origin = match self.frame.origin {
			ObservationOrigin::Run { run, .. } => format!("runs/{run}"),
			ObservationOrigin::Generation { request, .. } => format!("generation/{request}"),
		};
		BTreeMap::from([(
			format!(
				"{}/tenants/{}/approval-observer/{origin}/{}",
				self.frame.node, self.frame.tenant, record.decider.configuration_digest
			),
			record.attempt,
		)])
	}
	async fn begin(&self) -> Result<native::Transaction> {
		let mut tx = self
			.store
			.pool
			.begin(&TenancyScope::tenant(&self.frame.tenant)?)
			.await?;
		self.fence(&mut tx).await?;
		Ok(tx)
	}
	pub(super) async fn fence(&self, tx: &mut native::Transaction) -> Result<()> {
		let row = native::query(
			&Query::select()
				.columns(["frame", "worker", "lease_revision", "leased_until"].map(Alias::new))
				.from(Alias::new("approval_observations"))
				.and_where(Expr::col("id").eq(Expr::value(self.frame.id)))
				.lock(reinhardt::query::LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&mut **tx)
		.await?;
		let worker: Option<Uuid> = row.try_get("worker")?;
		let revision: i64 = row.try_get("lease_revision")?;
		let until: Option<DateTime<Utc>> = row.try_get("leased_until")?;
		let frame: Value = row.try_get("frame")?;
		if worker != Some(self.lease.worker)
			|| revision != self.lease.revision
			|| until.is_none_or(|v| v <= Utc::now())
			|| frame != json!(self.frame)
		{
			return Err(Error::Conflict("observation lease lost".into()));
		}
		Ok(())
	}
	fn boundary(&self, b: &Boundary) -> Result<()> {
		if *b != observation_boundary(&self.frame, &self.lease)? {
			return Err(Error::Conflict(
				"observation boundary differs from lease".into(),
			));
		}
		Ok(())
	}
	async fn attempts(
		&self,
		tx: &mut native::Transaction,
		decision: Uuid,
	) -> Result<Vec<RecoveredAttempt>> {
		let rows = native::query(
			&Query::select()
				.columns(["record", "status", "answers", "provider_report"].map(Alias::new))
				.from(Alias::new("approval_observation_attempts"))
				.and_where(Expr::col("observation").eq(Expr::value(self.frame.id)))
				.and_where(Expr::col("decision").eq(Expr::value(decision)))
				.order_by(Alias::new("created_at"), reinhardt::query::Order::Asc)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(&mut **tx)
		.await?;
		rows.into_iter()
			.map(|row| {
				let value: Value = row.try_get("record")?;
				let record = decode(&value)?;
				let status: String = row.try_get("status")?;
				let answers: Option<Value> = row.try_get("answers")?;
				let report: Option<Value> = row.try_get("provider_report")?;
				Ok(RecoveredAttempt {
					permit: DispatchPermit {
						owner_receipts: self.receipts(&record),
						record,
					},
					status: if status == "pending" {
						AttemptStatus::Uncertain
					} else {
						serde_json::from_value(json!(status))?
					},
					answers: answers.map(serde_json::from_value).transpose()?,
					provider_report: report
						.map(serde_json::from_value)
						.transpose()?
						.unwrap_or_default(),
				})
			})
			.collect()
	}
}
fn encode(r: &DispatchRecord) -> Value {
	json!({"attempt":r.attempt,"decision":r.decision,"boundary":r.boundary,"decider":r.decider,"binding_restrictions":r.binding_restrictions,"route":r.route,"request_digest":r.request_digest,"state_digest":r.state_digest,"questions":r.questions,"question_digest":r.question_digest,"mode":r.mode})
}
fn decode(v: &Value) -> Result<DispatchRecord> {
	Ok(DispatchRecord {
		attempt: serde_json::from_value(v["attempt"].clone())?,
		decision: serde_json::from_value(v["decision"].clone())?,
		boundary: serde_json::from_value(v["boundary"].clone())?,
		decider: serde_json::from_value(v["decider"].clone())?,
		binding_restrictions: serde_json::from_value(v["binding_restrictions"].clone())?,
		route: serde_json::from_value(v["route"].clone())?,
		request_digest: serde_json::from_value(v["request_digest"].clone())?,
		state_digest: serde_json::from_value(v["state_digest"].clone())?,
		questions: serde_json::from_value(v["questions"].clone())?,
		question_digest: serde_json::from_value(v["question_digest"].clone())?,
		mode: serde_json::from_value(v["mode"].clone())?,
	})
}
#[async_trait]
impl ObservationJournal for Journal {
	async fn completed(
		&self,
		frame: &ObservationFrame,
		lease: &ObservationLease,
	) -> aidash_application::Result<Option<ObservationEvidence>> {
		let mut tx = self.begin().await?;
		if frame != &self.frame || lease != &self.lease {
			return Err(aidash_application::Error::Conflict(
				"observation lease differs".into(),
			));
		}
		let row = repository::observation(&mut *tx, &frame.tenant, frame.approval)
			.await?
			.ok_or_else(|| Error::NotFound("observation".into()))?;
		let result = row
			.evidence
			.map(|e| serde_json::from_value(e.0))
			.transpose()?;
		tx.commit().await?;
		Ok(result)
	}
	async fn commit_observation(
		&self,
		frame: &ObservationFrame,
		lease: &ObservationLease,
		evidence: &ObservationEvidence,
	) -> aidash_application::Result<()> {
		evidence.validate(frame)?;
		if frame != &self.frame || lease != &self.lease {
			return Err(aidash_application::Error::Conflict(
				"observation lease differs".into(),
			));
		}
		let mut tx = self.begin().await?;
		let existing = repository::observation(&mut *tx, &frame.tenant, frame.approval)
			.await?
			.ok_or_else(|| Error::NotFound("observation".into()))?;
		if let Some(existing) = existing.evidence {
			if existing.0 != json!(evidence) {
				return Err(aidash_application::Error::Conflict(
					"immutable observation already completed".into(),
				));
			}
			return Ok(());
		}

		// Valid-looking answers cannot be forged at commit: each route replays its
		// exact durable physical attempts, whose answers were recorded by dispatch.
		for route in &evidence.routes {
			let mut answers = BTreeMap::new();
			for attempt in &route.attempts {
				let row = native::query(
					&Query::select()
						.columns(["record", "status", "answers", "provider_report"].map(Alias::new))
						.from(Alias::new("approval_observation_attempts"))
						.and_where(Expr::col("observation").eq(Expr::value(frame.id)))
						.and_where(Expr::col("id").eq(Expr::value(attempt.id)))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut *tx)
				.await?
				.ok_or(aidash_application::Error::Forbidden)?;
				let stored: Value = row.try_get("record")?;
				let record = decode(&stored)?;
				let status: String = row.try_get("status")?;
				let durable: Option<Value> = row.try_get("answers")?;
				let provider_report: Option<Value> = row.try_get("provider_report")?;
				let provider_report: AttemptProviderReport = provider_report
					.map(serde_json::from_value)
					.transpose()?
					.unwrap_or_default();
				let saved_status = if status == "pending" {
					AttemptStatus::Uncertain
				} else {
					serde_json::from_value(json!(status))?
				};
				if record.route != route.route
					|| record.state_digest != route.state_digest
					|| record.request_digest != attempt.request_digest
					|| record.questions != attempt.questions
					|| saved_status != attempt.status
					|| attempt.owner_receipts != self.receipts(&record)
					|| attempt.provider_report != provider_report
				{
					return Err(aidash_application::Error::Forbidden);
				}
				if saved_status == AttemptStatus::Answered {
					answers.extend(serde_json::from_value::<BTreeMap<String, Probability>>(
						durable.ok_or(aidash_application::Error::Forbidden)?,
					)?);
				}
			}
			if answers != route.answers {
				return Err(aidash_application::Error::Forbidden);
			}
		}
		native::query(
			&Query::update()
				.table(Alias::new("approval_observations"))
				.value(Alias::new("evidence"), json!(evidence))
				.and_where(Expr::col("id").eq(Expr::value(frame.id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		tx.commit().await?;
		Ok(())
	}
}
#[async_trait]
impl DecisionJournal for Journal {
	async fn recover(
		&self,
		decision: Uuid,
		boundary: &Boundary,
	) -> aidash_application::Result<Vec<RecoveredAttempt>> {
		self.boundary(boundary)?;
		let mut tx = self.begin().await?;
		let result = self.attempts(&mut tx, decision).await?;
		tx.commit().await?;
		Ok(result)
	}
	async fn reserve(&self, record: &DispatchRecord) -> aidash_application::Result<Reservation> {
		self.boundary(&record.boundary)?;
		let questions = observation_questions();
		if record.questions.is_empty()
			|| record
				.questions
				.iter()
				.any(|id| !questions.contains_key(id))
			|| aidash_domain::decision::question_digest(&questions, &record.questions)?
				!= record.question_digest
			|| aidash_application::decision::attempt_identity(
				record.decision,
				&record.request_digest,
				&record.questions,
			) != record.attempt
			|| aidash_application::decision::decision_identity(
				&record.boundary,
				&record.decider,
				&record.binding_restrictions,
				&record.state_digest,
				&[],
				0,
				record.route,
			) != record.decision
		{
			return Err(aidash_application::Error::Forbidden);
		}

		if Some(&record.decider) != self.frame.settings.decider.as_ref()
			|| record.mode != Mode::Shadow
			|| record.binding_restrictions != self.frame.settings.restrictions
		{
			return Err(aidash_application::Error::Forbidden);
		}
		let mut tx = self.begin().await?;
		// One allowance lock covers every observation of this Run and exact configuration.
		let owner = match self.frame.origin {
			ObservationOrigin::Run { run, .. } => run,
			_ => self.frame.approval,
		};
		let lock = format!("observer:{owner}:{}", record.decider.configuration_digest);
		let (sql, values) = Query::select()
			.expr(SimpleExpr::FunctionCall(
				"pg_advisory_xact_lock".into_iden(),
				vec![SimpleExpr::FunctionCall(
					"hashtextextended".into_iden(),
					vec![Expr::value(lock).into(), Expr::value(103_109i64).into()],
				)],
			))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?;
		let recovered = self.attempts(&mut tx, record.decision).await?;
		if let Some(prior) = recovered
			.into_iter()
			.find(|p| p.permit.record.attempt == record.attempt)
		{
			let mut saved = prior.permit.record.clone();
			saved.boundary.worker = record.boundary.worker;
			saved.boundary.run_revision = record.boundary.run_revision;
			if saved != *record {
				return Err(aidash_application::Error::Conflict(
					"charged observation changed".into(),
				));
			}
			tx.commit().await?;
			return Ok(Reservation::Recovered(prior));
		}
		let count: i64 = native::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from_as(Alias::new("approval_observation_attempts"), Alias::new("a"))
				.join(
					reinhardt::query::JoinType::InnerJoin,
					Alias::new("approval_observations"),
					Expr::cust("a.observation = approval_observations.id"),
				)
				.and_where(
					Expr::col((Alias::new("a"), Alias::new("tenant")))
						.eq(self.frame.tenant.clone()),
				)
				.and_where(
					if matches!(self.frame.origin, ObservationOrigin::Run { .. }) {
						Expr::col((Alias::new("approval_observations"), Alias::new("run_id")))
							.eq(Expr::value(owner))
					} else {
						Expr::col((Alias::new("approval_observations"), Alias::new("approval")))
							.eq(Expr::value(owner))
					},
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"a.record#>>'{decider,configuration_digest}' = ?".into(),
					vec![Expr::value(record.decider.configuration_digest.clone()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut *tx)
		.await?;
		let config = self
			.frame
			.settings
			.config
			.as_ref()
			.ok_or(aidash_application::Error::Forbidden)?;
		let limit = self
			.frame
			.settings
			.restrictions
			.max_calls_per_run
			.map_or(config.max_calls_per_run, |l| {
				l.min(config.max_calls_per_run)
			});
		if u64::try_from(count).unwrap_or(u64::MAX) >= limit {
			return Err(aidash_application::Error::Context(
				aidash_domain::context::recovery::Failure::DeciderCallsExhausted,
			));
		}
		native::query(
			&Query::insert()
				.into_table(Alias::new("approval_observation_attempts"))
				.columns(
					[
						"id",
						"tenant",
						"observation",
						"decision",
						"record",
						"status",
						"created_at",
					]
					.map(Alias::new),
				)
				.values_panic::<_, reinhardt::query::Value>([
					record.attempt.into(),
					self.frame.tenant.clone().into(),
					self.frame.id.into(),
					record.decision.into(),
					encode(record).into(),
					"pending".into(),
					Utc::now().into(),
				])
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		tx.commit().await?;
		Ok(Reservation::Fresh(DispatchPermit {
			record: record.clone(),
			owner_receipts: self.receipts(record),
		}))
	}

	async fn finish_attempt(
		&self,
		permit: &DispatchPermit,
		answers: Option<&BTreeMap<String, Probability>>,
		status: AttemptStatus,
	) -> aidash_application::Result<()> {
		self.finish_reported_attempt(permit, answers, status, &AttemptProviderReport::Unknown)
			.await
	}
	async fn finish_reported_attempt(
		&self,
		permit: &DispatchPermit,
		answers: Option<&BTreeMap<String, Probability>>,
		status: AttemptStatus,
		report: &AttemptProviderReport,
	) -> aidash_application::Result<()> {
		let mut tx = self.begin().await?;
		let prior = self
			.attempts(&mut tx, permit.record.decision)
			.await?
			.into_iter()
			.find(|a| a.permit.record.attempt == permit.record.attempt)
			.ok_or(aidash_application::Error::Forbidden)?;
		if prior.permit != *permit {
			return Err(aidash_application::Error::Forbidden);
		}
		let config = self
			.frame
			.settings
			.config
			.as_ref()
			.ok_or(aidash_application::Error::Forbidden)?;
		report.validate_route(
			config
				.routes
				.get(permit.record.route as usize)
				.ok_or(aidash_application::Error::Forbidden)?,
		)?;
		if (status == AttemptStatus::Answered) != answers.is_some() {
			return Err(aidash_application::Error::Forbidden);
		}
		if let Some(answers) = answers {
			validate_answers(
				&permit
					.record
					.questions
					.iter()
					.map(|id| {
						observation_questions()
							.get(id)
							.cloned()
							.map(|q| (id.clone(), q))
							.ok_or(aidash_application::Error::Forbidden)
					})
					.collect::<aidash_application::Result<_>>()?,
				answers,
			)?;
		}
		let raw_status: String = native::query_scalar(
			&Query::select()
				.column(Alias::new("status"))
				.from(Alias::new("approval_observation_attempts"))
				.and_where(Expr::col("id").eq(Expr::value(permit.record.attempt)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut *tx)
		.await?;
		if raw_status != "pending" {
			if prior.status != status
				|| prior.answers.as_ref() != answers
				|| prior.provider_report != *report
			{
				return Err(aidash_application::Error::Conflict(
					"immutable observation attempt result changed".into(),
				));
			}
			tx.commit().await?;
			return Ok(());
		}
		native::query(
			&Query::update()
				.table(Alias::new("approval_observation_attempts"))
				.value(
					Alias::new("status"),
					serde_json::to_value(status)?.as_str().unwrap(),
				)
				.value_expr(
					Alias::new("answers"),
					answers.map_or(Expr::cust("NULL"), |a| Expr::value(json!(a))),
				)
				.value(Alias::new("provider_report"), json!(report))
				.and_where(Expr::col("id").eq(Expr::value(permit.record.attempt)))
				.and_where(Expr::col("status").eq("pending"))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		tx.commit().await?;
		Ok(())
	}

	async fn commit(
		&self,
		_: &Evidence,
		_: Option<&Context>,
		_: Option<&RetainedState>,
	) -> aidash_application::Result<()> {
		Err(aidash_application::Error::Forbidden)
	}
	async fn commit_approval(&self, _: &ApprovalEvidence) -> aidash_application::Result<()> {
		Err(aidash_application::Error::Forbidden)
	}
	async fn expire_states(&self, _: DateTime<Utc>) -> aidash_application::Result<usize> {
		Ok(0)
	}
	async fn append_outcome(&self, _: &OutcomeLink) -> aidash_application::Result<()> {
		Err(aidash_application::Error::Forbidden)
	}
}
