//! Deployment-wide calibration samples for Request Estimates (ADR 0017). A
//! sample holds no Run, Tenant or attempt identifier: only its calibration
//! key, its byte basis and the provider's reported prompt tokens.
use crate::Result;
use aidash_domain::provider::estimate::{CALIBRATION_WINDOW, CalibrationSample};
use chrono::{DateTime, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{Model, OrmExecutor, execution::convert_values};
use reinhardt::model;
use reinhardt::query::{
	Alias, Expr, ExprTrait, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[model(app_label = "execution", table_name = "estimate_calibration_samples")]
#[derive(Serialize, Deserialize)]
pub struct EstimateCalibrationSample {
	#[field(primary_key = true, field_type = "uuid")]
	pub id: Uuid,
	/// The upstream model slug (`ModelConfig::model_id`).
	#[field(field_type = "text")]
	pub upstream_model: String,
	#[field]
	pub projection_version: i32,
	#[field]
	pub byte_basis: i64,
	#[field]
	pub prompt_tokens: i64,
	#[field]
	pub completed_at: DateTime<Utc>,
}

impl EstimateCalibrationSample {
	/// Record one sample in the caller's transaction and keep only the newest
	/// `CALIBRATION_WINDOW` samples of its key.
	pub(crate) async fn record(
		tx: &mut dyn TransactionExecutor,
		upstream_model: &str,
		projection_version: i32,
		byte_basis: i64,
		prompt_tokens: i64,
	) -> Result<()> {
		let sample = Self::build()
			// Time-ordered, so equal timestamps still sort newest first.
			.id(Uuid::now_v7())
			.upstream_model(upstream_model.to_owned())
			.projection_version(projection_version)
			.byte_basis(byte_basis)
			.prompt_tokens(prompt_tokens)
			.completed_at(Utc::now())
			.finish();
		Self::objects()
			.insert_with_executor(tx, &sample)
			.await
			.map_err(FrameworkError::from)?;
		let mut newest = Query::select();
		newest
			.column(Alias::new("id"))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("upstream_model").eq(Expr::value(upstream_model)))
			.and_where(Expr::col("projection_version").eq(Expr::value(projection_version)))
			.order_by(Alias::new("completed_at"), Order::Desc)
			.order_by(Alias::new("id"), Order::Desc)
			.limit(CALIBRATION_WINDOW as u64);
		let (sql, values) = Query::delete()
			.from_table(Alias::new(Self::table_name()))
			.and_where(Expr::col("upstream_model").eq(Expr::value(upstream_model)))
			.and_where(Expr::col("projection_version").eq(Expr::value(projection_version)))
			.and_where(Expr::col("id").not_in_subquery(newest.to_owned()))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}

	/// The newest samples of one key, newest first.
	pub(crate) async fn newest<E: OrmExecutor>(
		db: &mut E,
		upstream_model: &str,
		projection_version: i32,
	) -> Result<Vec<CalibrationSample>> {
		Ok(Self::objects()
			.filter(Self::field_upstream_model().eq(upstream_model.to_owned()))
			.filter(Self::field_projection_version().eq(projection_version))
			.order_by(&["-completed_at", "-id"])
			.limit(CALIBRATION_WINDOW)
			.all_with_db(db)
			.await?
			.into_iter()
			.map(|sample| CalibrationSample {
				byte_basis: u64::try_from(sample.byte_basis).unwrap_or_default(),
				prompt_tokens: u64::try_from(sample.prompt_tokens).unwrap_or_default(),
			})
			.collect())
	}
}
