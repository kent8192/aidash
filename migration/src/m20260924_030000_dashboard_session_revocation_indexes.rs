use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		for (name, column) in [
			("dashboard_session_identity", "identity_id"),
			("dashboard_session_provider_sid", "provider_sid"),
		] {
			manager
				.create_index(
					Index::create()
						.name(name)
						.table(Alias::new("dashboard_sessions"))
						.col(Alias::new(column))
						.to_owned(),
				)
				.await?;
		}
		Ok(())
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		for name in [
			"dashboard_session_provider_sid",
			"dashboard_session_identity",
		] {
			manager
				.drop_index(
					Index::drop()
						.name(name)
						.table(Alias::new("dashboard_sessions"))
						.to_owned(),
				)
				.await?;
		}
		Ok(())
	}
}
