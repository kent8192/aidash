use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		for (name, columns) in [
			("atomic_authority_pending", vec!["transaction_id"]),
			(
				"atomic_authority_pending_peer",
				vec!["node_id", "transaction_id"],
			),
		] {
			let mut index = Index::create();
			index
				.name(name)
				.table(Alias::new("atomic_authority_attempts"))
				.and_where(Expr::col(Alias::new("outcome")).is_null());
			for column in columns {
				index.col(Alias::new(column));
			}
			manager.create_index(index.to_owned()).await?;
		}
		Ok(())
	}
	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		for name in ["atomic_authority_pending_peer", "atomic_authority_pending"] {
			manager
				.drop_index(
					Index::drop()
						.name(name)
						.table(Alias::new("atomic_authority_attempts"))
						.to_owned(),
				)
				.await?;
		}
		Ok(())
	}
}
