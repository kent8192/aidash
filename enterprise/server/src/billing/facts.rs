//! Exact frozen metering facts. Nanoseconds never pass through a floating point
//! duration or PostgreSQL's microsecond timestamp representation.
use super::models::BillingCoveragePeriod;
use super::store::Deletion;
use aidash_server::{Error, Result};
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub(crate) fn nanos(at: DateTime<Utc>) -> Result<i64> {
	if at.timestamp_subsec_nanos() >= 1_000_000_000 {
		return Err(Error::Invalid(
			"leap-second metering evidence is unsupported".into(),
		));
	}
	at.timestamp_nanos_opt()
		.filter(|ns| *ns >= 0)
		.ok_or_else(|| {
			Error::Invalid(
				"metering time must be a nonnegative representable Unix nanosecond".into(),
			)
		})
}

pub(crate) fn span(start: DateTime<Utc>, end: DateTime<Utc>) -> Result<(i64, i64)> {
	let (start, end) = (nanos(start)?, nanos(end)?);
	if end <= start {
		return Err(Error::Invalid(
			"metering period must be positive and half-open".into(),
		));
	}
	Ok((start, end))
}

/// Floor explicitly at the legacy SQL timestamp boundary; the original is kept
/// in a separate integer column and remains the sole precise attribution input.
pub(crate) fn legacy_timestamp(ns: i64) -> DateTime<Utc> {
	DateTime::from_timestamp_micros(ns / 1000).expect("bounded epoch nanoseconds fit microseconds")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Piece {
	pub starts_ns: i64,
	pub ends_ns: i64,
	pub status: &'static str,
	pub coverage_id: Option<Uuid>,
	pub account_id: Option<Uuid>,
}

pub(crate) struct History {
	coverage: Vec<(i64, Option<i64>, Uuid, Uuid)>,
	deletions: Vec<(i64, Option<i64>)>,
}

impl History {
	pub fn new(
		coverage: Vec<BillingCoveragePeriod>,
		deletions: &[Deletion],
		birth: i64,
		now: i64,
	) -> Result<Self> {
		let mut periods = Vec::new();
		for p in coverage {
			let start = nanos(p.starts_at)?;
			let end = p.ends_at.map(nanos).transpose()?;
			if start < birth || start > now || end.is_some_and(|end| end <= start || end > now) {
				return Err(Error::Conflict(
					"contradictory or future coverage history".into(),
				));
			}
			periods.push((start, end, p.id, p.account_id));
		}
		periods.sort_by_key(|p| p.0);
		if periods
			.windows(2)
			.any(|p| p[0].1.is_none_or(|end| end > p[1].0))
		{
			return Err(Error::Conflict("overlapping coverage history".into()));
		}
		let mut normalized: Vec<(i64, Option<i64>)> = Vec::new();
		let mut spans = Vec::new();
		for &(start, end) in deletions {
			let start = nanos(start)?;
			let end = end.map(nanos).transpose()?;
			if start < birth || start > now || end.is_some_and(|end| end < start || end > now) {
				return Err(Error::Conflict(
					"contradictory or future Tenant deletion history".into(),
				));
			}
			spans.push((start, end));
		}
		spans.sort_by_key(|p| p.0);
		for (start, end) in spans {
			if let Some(last) = normalized.last_mut()
				&& last.1.is_none_or(|until| until >= start)
			{
				last.1 = match (last.1, end) {
					(Some(a), Some(z)) => Some(a.max(z)),
					_ => None,
				};
				continue;
			}
			normalized.push((start, end));
		}
		Ok(Self {
			coverage: periods,
			deletions: normalized,
		})
	}

	pub fn point(&self, at: i64) -> Piece {
		let (status, coverage_id, account_id) = if self
			.deletions
			.iter()
			.any(|(a, z)| *a <= at && z.is_none_or(|z| at < z))
		{
			("exempt", None, None)
		} else if let Some(p) = self
			.coverage
			.iter()
			.find(|p| p.0 <= at && p.1.is_none_or(|z| at < z))
		{
			("covered", Some(p.2), Some(p.3))
		} else {
			("uncovered", None, None)
		};
		Piece {
			starts_ns: at,
			ends_ns: at,
			status,
			coverage_id,
			account_id,
		}
	}

	pub fn partition(&self, start: i64, end: i64) -> Result<Vec<Piece>> {
		let mut boundaries = vec![start, end];
		for (a, z) in self
			.coverage
			.iter()
			.map(|p| (p.0, p.1))
			.chain(self.deletions.iter().copied())
		{
			if start < a && a < end {
				boundaries.push(a);
			}
			if let Some(z) = z
				&& start < z && z < end
			{
				boundaries.push(z);
			}
		}
		boundaries.sort();
		boundaries.dedup();
		let mut pieces: Vec<Piece> = Vec::new();
		for window in boundaries.windows(2) {
			let mut next = self.point(window[0]);
			next.ends_ns = window[1];
			if let Some(last) = pieces.last_mut()
				&& last.status == next.status
				&& last.coverage_id == next.coverage_id
				&& last.account_id == next.account_id
			{
				last.ends_ns = next.ends_ns;
				continue;
			}
			pieces.push(next);
		}
		let total: i128 = pieces
			.iter()
			.map(|p| i128::from(p.ends_ns - p.starts_ns))
			.sum();
		if total != i128::from(end - start)
			|| pieces.first().is_none_or(|p| p.starts_ns != start)
			|| pieces.last().is_none_or(|p| p.ends_ns != end)
		{
			return Err(Error::Invalid(
				"frozen usage partition does not conserve its raw interval".into(),
			));
		}
		Ok(pieces)
	}
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct AccountSegment {
	pub allocation: Uuid,
	pub tenant: String,
	pub coverage_id: Uuid,
	pub starts_at: DateTime<Utc>,
	pub ends_at: DateTime<Utc>,
	/// Decimal nanoseconds, including sub-microsecond quantities, without JSON number rounding.
	pub nanos: String,
}
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct AccountSample {
	pub id: Uuid,
	pub tenant: String,
	pub coverage_id: Uuid,
	pub sampled_at: DateTime<Utc>,
	pub bytes: u64,
}
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct AccountUsage {
	pub account: Uuid,
	pub starts_at: DateTime<Utc>,
	pub ends_at: DateTime<Utc>,
	pub pod_nanos: String,
	pub segments: Vec<AccountSegment>,
	pub samples: Vec<AccountSample>,
}
