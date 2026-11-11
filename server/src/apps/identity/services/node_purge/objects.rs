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
		"outbound" if !data["output_file"].is_null() => {
			values.push(&data["output_file"]);
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
	let mut selected = tx
		.fetch_all(&sql, convert_values(values))
		.await?
		.into_iter()
		.map(|row| {
			row.get("id")
				.map_err(reinhardt::core::exception::Error::from)
				.map_err(Into::into)
		})
		.collect::<Result<BTreeSet<Uuid>>>()?;
	// Deleting either object cascades its attachment row. Capture the whole
	// connected set before erasing any file, including shared WAV renditions.
	let mut frontier = selected.iter().copied().collect::<Vec<_>>();
	while !frontier.is_empty() {
		let references = selectors::ids(Expr::col("object_id").into(), &frontier)
			.or(selectors::ids(Expr::col("wav_object_id").into(), &frontier));
		let (sql, values) = Query::select()
			.columns(["object_id", "wav_object_id"].map(Alias::new))
			.from(Alias::new("channel_attachments"))
			.and_where(Expr::col("tenant").eq(Expr::value(owners.tenant)))
			.and_where(references.clone())
			.and_where(selectors::not_true(
				owners.predicate("channel_attachments")?,
			))
			.limit(1)
			.build(PostgresQueryBuilder);
		if tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.is_some()
		{
			return Err(Error::Conflict(
				"Node purge found an attachment object shared with a surviving owner".into(),
			));
		}
		let (sql, values) = Query::select()
			.columns(["object_id", "wav_object_id"].map(Alias::new))
			.from(Alias::new("channel_attachments"))
			.and_where(owners.predicate("channel_attachments")?)
			.and_where(references)
			.build(PostgresQueryBuilder);
		let mut next = Vec::new();
		for row in tx.fetch_all(&sql, convert_values(values)).await? {
			let row = QueryRow::from_backend_row(row).data;
			for field in ["object_id", "wav_object_id"] {
				let value: Option<Uuid> = serde_json::from_value(row[field].clone())?;
				if let Some(id) = value
					&& selected.insert(id)
				{
					next.push(id);
				}
			}
		}
		frontier = next;
	}
	if !selected.is_empty() {
		let ids = selected.iter().copied().collect::<Vec<_>>();
		let (sql, values) = Query::select()
			.expr(Expr::value(1_i32))
			.from(Alias::new("core_objects"))
			.and_where(Expr::col("tenant").eq(Expr::value(owners.tenant)))
			.and_where(selectors::ids(Expr::col("id").into(), &ids))
			.and_where(Expr::col("area_id").is_not_null())
			.and_where(selectors::not_true(owners.root(
				"core_objects",
				"area_id",
				crate::database::classification::node::Root::Area,
			)?))
			.limit(1)
			.build(PostgresQueryBuilder);
		if tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.is_some()
		{
			return Err(Error::Conflict(
				"Node purge found an attachment object owned by another area".into(),
			));
		}
	}
	Ok(selected.into_iter().collect())
}
/// A revision or receipt reference restricts its object. A surviving owner's
/// reference blocks the purge; the purged owner's references go with the batch.
async fn release_references(
	tx: &mut dyn TransactionExecutor,
	owners: &Owners<'_>,
	batch: &[Uuid],
) -> Result<()> {
	let selected = selectors::ids(Expr::col("object_id").into(), batch);
	let (sql, values) = Query::select()
		.expr(Expr::value(1_i32))
		.from(Alias::new("core_blob_references"))
		.and_where(Expr::col("tenant").eq(Expr::value(owners.tenant)))
		.and_where(selected.clone())
		.and_where(selectors::not_true(
			owners.predicate("core_blob_references")?,
		))
		.limit(1)
		.build(PostgresQueryBuilder);
	if tx
		.fetch_optional(&sql, convert_values(values))
		.await?
		.is_some()
	{
		return Err(Error::Conflict(
			"Node purge found a file referenced by a surviving owner".into(),
		));
	}
	let (sql, values) = Query::delete()
		.from_table(Alias::new("core_blob_references"))
		.and_where(Expr::col("tenant").eq(Expr::value(owners.tenant)))
		.and_where(selected)
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
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
		// The object rows committed in earlier batches. Blob bytes are shared
		// across the Tenant, so only digests nothing names any more are erased.
		let digests = crate::apps::execution::repositories::reclamation::unreferenced_blobs(
			&mut **tx,
			owners.tenant,
			OBJECT_BATCH,
		)
		.await?;
		if digests.is_empty() {
			return Ok(true);
		}
		// The purge driver commits each batch before the next: reclaim a single
		// digest so object-store I/O never spans several quota updates.
		let digest = &digests[0];
		let reclaimed = crate::apps::execution::repositories::reclamation::reclaim_blob(
			store,
			tx,
			owners.tenant,
			digest,
		)
		.await?;
		// A busy candidate belongs to the ordinary sweep; do not spin while
		// retaining the Node purge's fencing locks.
		return Ok(!matches!(
			reclaimed,
			crate::apps::execution::repositories::reclamation::BlobReclamation::Reclaimed
		));
	}
	release_references(&mut **tx, owners, &batch).await?;
	for id in batch {
		store
			.capabilities
			.erase_committed(tx, owners.tenant, id, true)
			.await?;
	}
	Ok(false)
}
