//! One Usage Record per Inference Attempt. Reported counts, provider cost and the
//! request estimate stay separate; unknown values stay NULL.
use crate::Result;
use aidash_domain::provider::usage::{UsageDispatch, UsageOutcome};
use chrono::{DateTime, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{Model, execution::convert_values};
use reinhardt::model;
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const DISPATCHED: &str = "dispatched";

#[model(app_label = "execution", table_name = "inference_usage")]
#[derive(Serialize, Deserialize)]
pub struct InferenceUsage {
	#[field(primary_key = true, field_type = "uuid")]
	pub attempt_id: Uuid,
	#[field]
	pub run_id: Uuid,
	#[field]
	pub lease_token: Uuid,
	#[field]
	pub response_epoch: i64,
	#[field(field_type = "text")]
	pub model_id: String,
	#[field(field_type = "text")]
	pub model_version: String,
	#[field]
	pub projection_version: i32,
	#[field(null = true)]
	pub input_tokens: Option<i64>,
	#[field(null = true)]
	pub output_tokens: Option<i64>,
	#[field(null = true)]
	pub cache_read_tokens: Option<i64>,
	#[field(null = true)]
	pub cache_write_tokens: Option<i64>,
	#[field(null = true)]
	pub reasoning_tokens: Option<i64>,
	#[field(null = true)]
	pub cost_nanocredits: Option<i64>,
	#[field(null = true)]
	pub upstream_cost_nanocredits: Option<i64>,
	#[field]
	pub estimated_tokens: i64,
	#[field(field_type = "text")]
	pub estimator: String,
	#[field]
	pub estimator_version: i32,
	#[field(field_type = "text")]
	pub estimate_confidence: String,
	/// The upstream model slug, the calibration key; NULL before #179.
	#[field(field_type = "text", null = true)]
	pub upstream_model: Option<String>,
	/// Serialized request body plus cache-salt reserve, in bytes.
	#[field(null = true)]
	pub byte_basis: Option<i64>,
	/// The output reservation inside `estimated_tokens`.
	#[field(null = true)]
	pub output_reservation: Option<i64>,
	/// The Estimate Snapshot's ratio in parts per million; NULL for bytes.
	#[field(null = true)]
	pub estimate_ratio_ppm: Option<i64>,
	/// A complete outcome becomes a Deployment-wide calibration sample.
	#[field]
	pub calibration_sample: bool,
	/// The Stable Prefix cache identity of a salted request; NULL for Legacy.
	#[field(field_type = "text", null = true)]
	pub cache_identity: Option<String>,
	#[field(field_type = "text")]
	pub outcome: String,
	#[field(field_type = "text", null = true)]
	pub rejection_class: Option<String>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	#[field(null = true)]
	pub completed_at: Option<DateTime<Utc>>,
}

/// Counts beyond the database range are unknown rather than clamped.
fn stored(count: Option<u64>) -> Option<i64> {
	count.and_then(|count| i64::try_from(count).ok())
}

impl InferenceUsage {
	/// The caller holds the Run's worker lease. Records the Run left dispatched
	/// belong to a lost lease or crash, so they become unknown before insertion.
	pub(crate) async fn dispatch(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		dispatch: &UsageDispatch,
	) -> Result<()> {
		Self::abandon(tx, run, None).await?;
		let record = Self::build()
			.attempt_id(dispatch.attempt)
			.run_id(run)
			.lease_token(dispatch.lease)
			.response_epoch(dispatch.response_epoch)
			.model_id(dispatch.model_id.clone())
			.model_version(dispatch.model_version.clone())
			.projection_version(i32::from(dispatch.projection_version))
			.input_tokens(None)
			.output_tokens(None)
			.cache_read_tokens(None)
			.cache_write_tokens(None)
			.reasoning_tokens(None)
			.cost_nanocredits(None)
			.upstream_cost_nanocredits(None)
			.estimated_tokens(i64::try_from(dispatch.estimate.tokens).unwrap_or(i64::MAX))
			.estimator(dispatch.estimate.estimator.clone())
			.estimator_version(i32::try_from(dispatch.estimate.version).unwrap_or(i32::MAX))
			.estimate_confidence(dispatch.estimate.confidence.as_str().to_owned())
			.upstream_model(Some(dispatch.upstream_model.clone()))
			.byte_basis(stored(Some(dispatch.estimate.byte_basis)))
			.output_reservation(stored(Some(dispatch.estimate.output_reservation)))
			.estimate_ratio_ppm(dispatch.estimate.ratio_ppm.map(i64::from))
			.calibration_sample(dispatch.calibration_sample)
			.cache_identity(dispatch.cache_identity.clone())
			.outcome(DISPATCHED.to_owned())
			.rejection_class(None)
			.completed_at(None)
			.finish();
		Self::objects()
			.insert_with_executor(tx, &record)
			.await
			.map_err(FrameworkError::from)?;
		Ok(())
	}

	/// A new lease claim supersedes every other lease's dispatched attempt: its
	/// worker crashed or lost the lease, so the attempt becomes unknown even if
	/// the Run never infers again. A late completion is then no longer written.
	pub(crate) async fn abandon_superseded(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		lease: Uuid,
	) -> Result<()> {
		Self::abandon(tx, run, Some(lease)).await
	}

	/// Make the Run's dispatched attempts unknown, except those of `kept`.
	async fn abandon(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		kept: Option<Uuid>,
	) -> Result<()> {
		let mut update = Query::update();
		update
			.table(Alias::new(Self::table_name()))
			.value_expr(
				Alias::new("outcome"),
				Expr::value(UsageOutcome::Unknown.as_str()),
			)
			.value_expr(Alias::new("completed_at"), Expr::current_timestamp())
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("outcome").eq(Expr::value(DISPATCHED)));
		if let Some(kept) = kept {
			update.and_where(Expr::col("lease_token").ne(Expr::value(kept)));
		}
		let (sql, values) = update.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}

	/// Complete a still-dispatched record once. Returns whether it was written.
	/// A complete outcome of a calibration-sample attempt also records its
	/// Deployment-wide sample in the same transaction (ADR 0017).
	pub(crate) async fn complete(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		attempt: Uuid,
		outcome: &UsageOutcome,
	) -> Result<bool> {
		let mut update = Query::update();
		update
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("outcome"), Expr::value(outcome.as_str()))
			.value_expr(Alias::new("completed_at"), Expr::current_timestamp());
		match outcome {
			UsageOutcome::Completed(usage) => {
				let cost = usage.cost;
				for (column, value) in [
					("input_tokens", stored(usage.input_tokens)),
					("output_tokens", stored(usage.output_tokens)),
					("cache_read_tokens", stored(usage.cache_read_tokens)),
					("cache_write_tokens", stored(usage.cache_write_tokens)),
					("reasoning_tokens", stored(usage.reasoning_tokens)),
					("cost_nanocredits", cost.map(|cost| cost.nanocredits)),
					(
						"upstream_cost_nanocredits",
						cost.and_then(|cost| cost.upstream_nanocredits),
					),
				] {
					update.value_expr(Alias::new(column), Expr::value(value));
				}
			}
			UsageOutcome::Rejected { class } => {
				update.value_expr(Alias::new("rejection_class"), Expr::value(class.as_str()));
			}
			UsageOutcome::Unknown => {}
		}
		let (sql, values) = update
			.and_where(Expr::col("attempt_id").eq(Expr::value(attempt)))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("outcome").eq(Expr::value(DISPATCHED)))
			.build(PostgresQueryBuilder);
		let written = tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected
			== 1;
		if written && matches!(outcome, UsageOutcome::Completed(_)) {
			Self::record_calibration_sample(tx, attempt).await?;
		}
		Ok(written)
	}

	async fn record_calibration_sample(
		tx: &mut dyn TransactionExecutor,
		attempt: Uuid,
	) -> Result<()> {
		let records = Self::objects()
			.filter(Self::field_attempt_id().eq(attempt))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?;
		let Some(record) = records.first().filter(|record| record.calibration_sample) else {
			return Ok(());
		};
		if let (Some(model), Some(byte_basis), Some(prompt_tokens)) = (
			&record.upstream_model,
			record.byte_basis,
			record.input_tokens,
		) && byte_basis > 0
			&& prompt_tokens > 0
		{
			super::estimate_calibration::EstimateCalibrationSample::record(
				tx,
				model,
				record.projection_version,
				byte_basis,
				prompt_tokens,
			)
			.await?;
		}
		Ok(())
	}
}
