//! Research-owned exact journal variants. The journal stores only their references.
use super::*;
use aidash_domain::context::{ContextEvent, HistoryEntry, research::ResearchReference};
use aidash_domain::research::expiry::ExpiredResults;

#[model(app_label = "execution", table_name = "research_journal_results")]
#[derive(Serialize, Deserialize)]
pub struct ResearchJournalResult {
	#[field(primary_key = true, field_type = "uuid")]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub run_id: Uuid,
	#[field(field_type = "text")]
	pub invocation_key: String,
	#[field(field_type = "text")]
	pub digest: String,
	#[field]
	pub reference: Json<Value>,
	#[field(null = true)]
	pub bytes: Option<Vec<u8>>,
	#[field(field_type = "text", null = true)]
	pub purge_reason: Option<String>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

impl ResearchJournalResult {
	async fn get(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		reference: &ResearchReference,
	) -> Result<Option<Self>> {
		Ok(Self::objects()
			.filter(Self::field_run_id().eq(run))
			.filter(Self::field_invocation_key().eq(reference.invocation.clone()))
			.filter(Self::field_digest().eq(reference.digest.clone()))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop())
	}
	/// Called under the Run row and Research lock. Existing tombstones are
	/// never overwritten, even by a late worker holding the original bytes.
	pub(crate) async fn freeze(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		entries: &[HistoryEntry],
		expired: &ExpiredResults,
		revoked: bool,
	) -> Result<()> {
		for entry in entries {
			let Some(reference) = entry.research_reference() else {
				continue;
			};
			let (sql, values) = Query::select()
				.column(Alias::new("id"))
				.from(Alias::new(Self::table_name()))
				.and_where(Expr::col("run_id").eq(Expr::value(run)))
				.and_where(
					Expr::col("invocation_key").eq(Expr::value(reference.invocation.clone())),
				)
				.and_where(Expr::col("digest").eq(Expr::value(reference.digest.clone())))
				.build(PostgresQueryBuilder);
			if tx
				.fetch_optional(&sql, convert_values(values))
				.await?
				.is_some()
			{
				continue;
			}
			let ContextEvent::Tool { result, .. } = &entry.event else {
				continue;
			};
			let terminated = ResearchAttempt::expired_tombstone(tx, run, &reference.invocation)
				.await?
				.is_some();
			let reason = if revoked || ContextEvent::is_revoked_result(result) {
				Some("revocation")
			} else if terminated
				|| reference.covered_by(expired)
				|| ContextEvent::is_expired_result(result)
			{
				Some("account_terms")
			} else {
				None
			};
			let bytes = if reason.is_some()
				|| ResearchReference::parse(result).is_some()
				|| result["contract"] == "aidash.research.unavailable/1"
			{
				None
			} else {
				Some(serde_json::to_vec(result)?)
			};
			// An unresolved/elided reference cannot manufacture a new payload.
			let (sql, values) = Query::select()
				.column(Alias::new("tenant"))
				.from(Alias::new("runs"))
				.and_where(Expr::col("id").eq(Expr::value(run)))
				.build(PostgresQueryBuilder);
			let row = tx.fetch_one(&sql, convert_values(values)).await?;
			let tenant: String =
				serde_json::from_value(QueryRow::from_backend_row(row).data["tenant"].clone())?;
			let record = Self::new()
				.id(Uuid::new_v4())
				.tenant(tenant)
				.run_id(run)
				.invocation_key(reference.invocation.clone())
				.digest(reference.digest.clone())
				.reference(Json(serde_json::to_value(&reference)?))
				.bytes(bytes)
				.purge_reason(reason.map(str::to_owned))
				.finish();
			Self::objects()
				.insert_with_executor(tx, &record)
				.await
				.map_err(FrameworkError::from)?;
		}
		Ok(())
	}
	/// Resolve only stored references; a fresh tool event already holds its
	/// exact delivered value. Never serialize these ephemeral values durably.
	pub(crate) async fn resolve(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		entries: &mut [HistoryEntry],
		expired: &ExpiredResults,
		revoked: bool,
	) -> Result<()> {
		for entry in entries {
			let ContextEvent::Tool { call, result } = &mut entry.event else {
				continue;
			};
			let Some(reference) = ResearchReference::parse(result) else {
				continue;
			};
			let record = Self::get(tx, run, &reference).await?;
			let reason = record
				.as_ref()
				.and_then(|record| record.purge_reason.as_deref());
			*result = if revoked || reason == Some("revocation") {
				ContextEvent::revoked_result(&call.id)
			} else if reference.covered_by(expired) || reason == Some("account_terms") {
				ContextEvent::expired_result(&call.id)
			} else if let Some(record) = record {
				if record.reference.0 != serde_json::to_value(&reference)? {
					return Err(Error::Invalid(
						"Research journal reference metadata mismatch".into(),
					));
				}
				if let Some(bytes) = record.bytes {
					use sha2::{Digest, Sha256};
					if format!("sha256:{:x}", Sha256::digest(&bytes)) != reference.digest {
						return Err(Error::Invalid(
							"Research journal payload digest mismatch".into(),
						));
					}
					serde_json::from_slice(&bytes)?
				} else {
					reference.unavailable()
				}
			} else {
				reference.unavailable()
			};
			entry.research = Some(reference);
		}
		Ok(())
	}
	pub(crate) async fn revoke(tx: &mut dyn TransactionExecutor, run: Uuid) -> Result<()> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value(Alias::new("bytes"), Option::<Vec<u8>>::None)
			.value(Alias::new("purge_reason"), "revocation")
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}
	pub(crate) async fn expire(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		keys: &[String],
		expired: &ExpiredResults,
	) -> Result<()> {
		// Read metadata only, including orphan variants no longer projected.
		let (sql, values) = Query::select()
			.columns(["id", "reference"].map(Alias::new))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("bytes").is_not_null())
			.build(PostgresQueryBuilder);
		let mut ids = Vec::new();
		for row in tx.fetch_all(&sql, convert_values(values)).await? {
			let data = QueryRow::from_backend_row(row).data;
			let reference = ResearchReference::parse(&data["reference"])
				.ok_or_else(|| Error::Invalid("invalid Research journal reference".into()))?;
			if keys.contains(&reference.invocation) || reference.covered_by(expired) {
				ids.push(serde_json::from_value::<Uuid>(data["id"].clone())?);
			}
		}
		if !ids.is_empty() {
			let (sql, values) = Query::update()
				.table(Alias::new(Self::table_name()))
				.value(Alias::new("bytes"), Option::<Vec<u8>>::None)
				.value(Alias::new("purge_reason"), "account_terms")
				.and_where(Expr::col("id").is_in(ids.into_iter().map(Expr::value)))
				.build(PostgresQueryBuilder);
			tx.execute(&sql, convert_values(values)).await?;
		}
		Ok(())
	}
}
