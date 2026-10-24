//! Native Reinhardt migration execution for this service only.
use crate::{Error, Result};
use reinhardt::db::migrations::FieldType;
use reinhardt::db::{
	backends::DatabaseConnection,
	migrations::{DatabaseMigrationExecutor, DatabaseMigrationRecorder, prelude::*},
};
pub fn initial() -> Migration {
	Migration::new("0001_initial", "marketplace_distribution")
		.add_operation(Operation::CreateTable {
			name: "marketplace_documents".into(),
			columns: vec![
				ColumnDefinition::new("key", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("document", FieldType::Jsonb).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.atomic(true)
		.with_initial(Some(true))
}
// Migration bootstrap must use this independent service's schema-owner role,
// which cannot be composed through Cloud's Tenant-scoped Pool. This connection
// is local to the Marketplace migration executor and is never a global default.
#[allow(clippy::disallowed_methods)]
pub async fn migrate(url: &str) -> Result<()> {
	let _schema_check = crate::store::Store::connect(url).await?;
	let db = DatabaseConnection::connect_postgres(url)
		.await
		.map_err(|_| Error::Unavailable)?;
	DatabaseMigrationExecutor::new(db.clone())
		.apply_migrations(&[initial()])
		.await
		.map_err(|_| Error::Unavailable)?;
	db.execute(include_str!("../seal-ledger.sql"), Vec::new())
		.await
		.map_err(|_| Error::Unavailable)?;
	Ok(())
}
// Startup inspects only this service's migration ledger with its own role.
// Cloud's Tenant-scoped Pool cannot compose that independent schema; this local
// inspection connection is never registered as a global/default connection.
#[allow(clippy::disallowed_methods)]
pub async fn require_current(url: &str) -> Result<()> {
	let db = DatabaseConnection::connect_postgres(url)
		.await
		.map_err(|_| Error::Unavailable)?;
	let applied = DatabaseMigrationRecorder::new(db)
		.get_applied_migrations_if_present()
		.await
		.map_err(|_| Error::Unavailable)?;
	if applied.len() != 1
		|| applied[0].app != "marketplace_distribution"
		|| applied[0].name != "0001_initial"
	{
		return Err(Error::Invalid(
			"Marketplace migrations are not current; run migrate with its owner credential",
		));
	}
	Ok(())
}
