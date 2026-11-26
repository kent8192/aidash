//! Drive background jobs share the worker lifetime and resume from persisted rows.
use super::{index, jobs, repository as db, tools::MAINTENANCE};
use crate::{Error, Result, store::Store};
use aidash_application::{drive::Change, tenancy::TenancyScope};
use chrono::Utc;
use reinhardt::query::{Alias, Expr, ExprTrait, LockType, Query};
use serde::Deserialize;
use std::{collections::BTreeMap, time::Duration};
use uuid::Uuid;

pub(crate) async fn run(
	store: Store,
	mut stopping: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
	let mut sweep_at = tokio::time::Instant::now();
	while !*stopping.borrow() {
		if let Err(error) = jobs::tick(&store).await {
			tracing::warn!(%error, "Drive publication sweep failed");
		}
		if let Err(error) = index::tick(&store).await {
			tracing::warn!(%error, "Drive indexing sweep failed");
		}
		if tokio::time::Instant::now() >= sweep_at {
			if let Err(error) = retention(&store).await {
				tracing::warn!(%error, "Drive retention sweep failed");
			}
			sweep_at = tokio::time::Instant::now() + Duration::from_secs(60);
		}
		tokio::select! {
			_ = stopping.changed() => {},
			_ = tokio::time::sleep(Duration::from_secs(2)) => {},
		}
	}
	Ok(())
}

#[derive(Deserialize)]
struct Tenant {
	tenant: String,
}

async fn retention(store: &Store) -> Result<()> {
	let query = Query::select()
		.column(Alias::new("tenant"))
		.distinct()
		.from(Alias::new("drives"))
		.to_owned();
	let tenants = store
		.pool
		.atomic(&MAINTENANCE, async |tx| {
			db::rows::<Tenant>(tx, &query).await
		})
		.await?;
	for Tenant { tenant } in tenants {
		let scope = TenancyScope::tenant(&tenant)?;
		let result = store
			.pool
			.atomic(&scope, async |tx| retain_tenant(tx, &tenant).await)
			.await;
		if let Err(error) = result
			&& !matches!(error, Error::TenantDeleted)
		{
			tracing::warn!(%tenant, %error, "Drive retention failed for Tenant");
		}
	}
	Ok(())
}

async fn retain_tenant(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	tenant: &str,
) -> Result<()> {
	use crate::apps::identity::models::AuthorizationBundle;
	use aidash_domain::{
		drive::{BaseVersion, PublishBase},
		policy::SubjectKind,
	};
	let snapshot = AuthorizationBundle::lock_snapshot(tx, tenant, false).await?;
	let now = Utc::now();
	#[derive(Deserialize)]
	struct Expired {
		id: Uuid,
		entry_id: Uuid,
		drive_id: Uuid,
	}
	#[derive(Deserialize)]
	struct Upload {
		id: Uuid,
	}
	let drives = db::rows::<db::Drive>(tx, &db::select("drives", tenant)).await?;
	for drive in drives {
		let query = db::select("drives", tenant)
			.and_where(db::col("id").eq(Expr::value(drive.id)))
			.lock(LockType::Update)
			.to_owned();
		let Some(drive) = db::rows::<db::Drive>(tx, &query).await?.pop() else {
			continue;
		};
		let mut query = db::snapshot(&drive, tenant, drive.head)?;
		query.and_where(db::col("path").like("private/%"));
		let mut paths = BTreeMap::new();
		let mut changes = Vec::new();
		for entry in db::rows::<db::Entry>(tx, &query).await? {
			let path = entry.file.path;
			let owner = aidash_domain::drive::private_owner(&path).unwrap_or_default();
			if !snapshot
				.bundle
				.subjects
				.get(owner)
				.is_some_and(|subject| subject.enabled && subject.kind == SubjectKind::User)
			{
				paths.insert(path.clone(), BaseVersion::Version(drive.head));
				changes.push(Change::Trash { path });
			}
		}
		if !changes.is_empty() {
			let system = db::Grant {
				tenant: tenant.into(),
				person: "drive-retention".into(),
				administrator: true,
				may_request_work: true,
			};
			db::publish(
				tx,
				&drive,
				&system,
				&PublishBase::Paths(paths),
				changes,
				db::Provenance {
					author: &system.person,
					requester: &system.person,
					task_id: None,
					run_id: None,
					operation_id: None,
				},
			)
			.await?;
		}
		let historical = db::select("drive_entries", tenant)
			.and_where(db::col("drive_id").eq(Expr::value(drive.id)))
			.and_where(db::col("path").like("private/%"))
			.and_where(db::col("removed_in_version").is_not_null())
			.and_where(db::col("owner_departed_at").is_null())
			.to_owned();
		for entry in db::rows::<db::Entry>(tx, &historical).await? {
			let owner = aidash_domain::drive::private_owner(&entry.file.path).unwrap_or_default();
			if snapshot
				.bundle
				.subjects
				.get(owner)
				.is_some_and(|subject| subject.enabled && subject.kind == SubjectKind::User)
			{
				continue;
			}
			#[derive(Deserialize)]
			struct TrashId {
				id: Uuid,
			}
			let expires = now + chrono::Duration::days(aidash_domain::drive::TRASH_DAYS);
			let query = db::select("drive_trash", tenant)
				.and_where(db::col("entry_id").eq(Expr::value(entry.id)))
				.to_owned();
			if let Some(trash) = db::rows::<TrashId>(tx, &query).await?.pop() {
				db::update(
					tx,
					"drive_trash",
					tenant,
					trash.id,
					[("expires_at", Expr::value(expires).into())],
				)
				.await?;
			} else if let Some(removed) = entry.removed_in_version {
				db::insert(
					tx,
					"drive_trash",
					[
						"id",
						"tenant",
						"drive_id",
						"path",
						"entry_id",
						"removed_in_version",
						"expires_at",
					],
					[
						Uuid::new_v4().into(),
						tenant.into(),
						drive.id.into(),
						entry.file.path.into(),
						entry.id.into(),
						removed.into(),
						expires.into(),
					],
				)
				.await?;
			}
			db::update(
				tx,
				"drive_entries",
				tenant,
				entry.id,
				[("owner_departed_at", Expr::value(now).into())],
			)
			.await?;
		}
		let query = db::select("drive_trash", tenant)
			.and_where(db::col("drive_id").eq(Expr::value(drive.id)))
			.and_where(db::col("expires_at").lte(Expr::value(now)))
			.limit(256)
			.to_owned();
		for expired in db::rows::<Expired>(tx, &query).await? {
			let query = db::select("drive_entries", tenant)
				.and_where(db::col("id").eq(Expr::value(expired.entry_id)))
				.to_owned();
			let Some(entry) = db::rows::<db::Entry>(tx, &query).await?.pop() else {
				continue;
			};
			let end = entry.removed_in_version.unwrap_or(i64::MAX);
			let pinned = db::select("drive_versions", tenant)
				.and_where(db::col("drive_id").eq(Expr::value(expired.drive_id)))
				.and_where(db::col("task_id").is_not_null())
				.and_where(db::col("version").gte(Expr::value(entry.file.added_in_version)))
				.and_where(db::col("version").lt(Expr::value(end)))
				.limit(1)
				.to_owned();
			// Completion Messages and Correction Requests retain the versions
			// published by their Tasks. Keep any entry in those snapshots.
			let pinned = !db::rows::<serde_json::Value>(tx, &pinned).await?.is_empty();
			db::delete(
				tx,
				"drive_trash",
				tenant,
				db::col("id").eq(Expr::value(expired.id)),
			)
			.await?;
			if !pinned {
				db::delete(
					tx,
					"drive_entries",
					tenant,
					db::col("id").eq(Expr::value(expired.entry_id)),
				)
				.await?;
			}
		}
	}
	let uploads = db::select("drive_uploads", tenant)
		.and_where(db::col("expires_at").lte(Expr::value(now)))
		.limit(256)
		.lock(LockType::Update)
		.to_owned();
	for upload in db::rows::<Upload>(tx, &uploads).await? {
		db::delete(
			tx,
			"drive_upload_parts",
			tenant,
			db::col("upload_id").eq(Expr::value(upload.id)),
		)
		.await?;
		db::delete(
			tx,
			"drive_uploads",
			tenant,
			db::col("id").eq(Expr::value(upload.id)),
		)
		.await?;
	}
	Ok(())
}
