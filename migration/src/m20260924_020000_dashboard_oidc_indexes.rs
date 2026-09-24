use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.create_index(
				Index::create()
					.name("dashboard_logout_expiry")
					.table(Alias::new("dashboard_logout_tokens"))
					.col(Alias::new("expires_at"))
					.to_owned(),
			)
			.await?;
		manager
			.create_index(
				Index::create()
					.name("dashboard_registration_pending")
					.table(Alias::new("dashboard_registration_requests"))
					.col(Alias::new("status"))
					.col(Alias::new("expires_at"))
					.to_owned(),
			)
			.await?;
		Ok(())
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		for (name, table) in [
			(
				"dashboard_registration_pending",
				"dashboard_registration_requests",
			),
			("dashboard_logout_expiry", "dashboard_logout_tokens"),
		] {
			manager
				.drop_index(Index::drop().name(name).table(Alias::new(table)).to_owned())
				.await?;
		}
		Ok(())
	}
}
