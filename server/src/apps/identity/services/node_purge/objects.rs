//! Physical files are erased while their exact database provenance still
//! exists. No object row enters the generic metadata DELETE step.
use super::*;
use aidash_domain::capabilities::operations::MountedFile;
use aidash_domain::capabilities::reclamation::provisional_kind;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

fn files(kind: &str, data: &Value) -> Result<Vec<MountedFile>> {
	let mut values = Vec::new();
	match kind {
		"reference" => {
			for key in ["original", "extraction"] {
				if !data[key].is_null() {
					values.push(&data[key]);
				}
			}
			if let Some(chunks) = data["chunks"].as_array() {
				for chunk in chunks {
					if !chunk["file"].is_null() {
						values.push(&chunk["file"]);
					}
				}
			}
		}
		"transfer_out" => {
			if let Some(files) = data["description"]["files"].as_array() {
				values.extend(files);
			}
		}
		"transfer_in" => {
			if let Some(chunks) = data["chunks"].as_object() {
				values.extend(chunks.values());
			}
		}
		"outbound" => {
			if !data["output_file"].is_null() {
				values.push(&data["output_file"]);
			}
		}
		_ => {}
	}
	values.into_iter().map(|value|serde_json::from_value(value.clone()).map_err(|_|Error::Conflict("Node purge found invalid provisional file provenance; repair it before retrying".into()))).collect()
}

/// Verified provisional IDs are also used by the read-only byte projection.
/// Present areas are authoritative; only area-less objects follow a typed
/// owned record. A shared record referencing the same object blocks erasure.
pub(super) async fn provisional(
	tx: &mut dyn TransactionExecutor,
	owners: &Owners<'_>,
) -> Result<BTreeSet<Uuid>> {
	let (sql, values) = Query::select()
		.columns(["id", "kind", "data"].map(Alias::new))
		.expr_as(owners.predicate("core_records")?, Alias::new("owned"))
		.from(Alias::new("core_records"))
		.and_where(Expr::col("tenant").eq(Expr::value(owners.tenant)))
		.and_where(Expr::col("kind").is_in([
			"reference",
			"transfer_in",
			"transfer_out",
			"outbound",
		]))
		.build(PostgresQueryBuilder);
	let mut owned = BTreeMap::<Uuid, Vec<(String, MountedFile)>>::new();
	let mut shared = BTreeSet::new();
	for row in tx.fetch_all(&sql, convert_values(values)).await? {
		let row = QueryRow::from_backend_row(row).data;
		let kind: String = serde_json::from_value(row["kind"].clone())?;
		let data: Value = row["data"].clone();
		let is_owned: Option<bool> = serde_json::from_value(row["owned"].clone())?;
		let descriptors = files(&kind, &data)?;
		for file in descriptors {
			if is_owned == Some(true) {
				owned
					.entry(file.file_id)
					.or_default()
					.push((kind.clone(), file));
			} else {
				shared.insert(file.file_id);
			}
		}
	}
	if owned.is_empty() {
		return Ok(BTreeSet::new());
	}
	let (sql, values) = Query::select()
		.columns(["id", "kind", "digest", "size"].map(Alias::new))
		.from(Alias::new("core_objects"))
		.and_where(Expr::col("tenant").eq(Expr::value(owners.tenant)))
		.and_where(Expr::col("area_id").is_null())
		.and_where(selectors::ids(
			Expr::col("id").into(),
			&owned.keys().copied().collect::<Vec<_>>(),
		))
		.build(PostgresQueryBuilder);
	let mut ids = BTreeSet::new();
	for row in tx.fetch_all(&sql, convert_values(values)).await? {
		let id: Uuid = row
			.get("id")
			.map_err(reinhardt::core::exception::Error::from)?;
		let kind: String = row
			.get("kind")
			.map_err(reinhardt::core::exception::Error::from)?;
		let digest: String = row
			.get("digest")
			.map_err(reinhardt::core::exception::Error::from)?;
		let size: i64 = row
			.get("size")
			.map_err(reinhardt::core::exception::Error::from)?;
		if shared.contains(&id) {
			return Err(Error::Conflict("Node purge found provisional file provenance shared with a surviving record; repair its owner before retrying".into()));
		}
		if owned[&id].iter().any(|(record, file)| {
			(provisional_kind(record, &kind) || (record == "outbound" && kind == "network_output"))
				&& file.digest == digest
				&& i64::try_from(file.size) == Ok(size)
		}) {
			ids.insert(id);
		} else {
			return Err(Error::Conflict(
				"Node purge cannot verify provisional file kind, size and digest".into(),
			));
		}
	}
	Ok(ids)
}
pub(super) async fn ids(
	tx: &mut dyn TransactionExecutor,
	owners: &Owners<'_>,
	limit: u64,
) -> Result<Vec<Uuid>> {
	let provisional = provisional(tx, owners).await?;
	let predicate = owners.predicate("core_objects")?.or(Expr::col("tenant")
		.eq(Expr::value(owners.tenant))
		.and(Expr::col("area_id").is_null())
		.and(selectors::ids(
			Expr::col("id").into(),
			&provisional.into_iter().collect::<Vec<_>>(),
		)));
	let (sql, values) = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("core_objects"))
		.and_where(predicate)
		.order_by(Alias::new("id"), Order::Asc)
		.limit(limit)
		.lock(LockType::Update)
		.build(PostgresQueryBuilder);
	tx.fetch_all(&sql, convert_values(values))
		.await?
		.into_iter()
		.map(|r| {
			r.get("id")
				.map_err(reinhardt::core::exception::Error::from)
				.map_err(Into::into)
		})
		.collect()
}
pub(super) async fn erase(
	store: &Store,
	tx: &mut native::Transaction,
	owners: &Owners<'_>,
) -> Result<bool> {
	// Caller retains gate Share NOWAIT, policy/capacity/deletion locks and the
	// recovery/preflight checks before this first filesystem operation.
	let batch = ids(&mut **tx, owners, OBJECT_BATCH).await?;
	if batch.is_empty() {
		return Ok(true);
	}
	for id in batch {
		store
			.capabilities
			.erase_committed(tx, owners.tenant, id)
			.await?;
	}
	Ok(false)
}
