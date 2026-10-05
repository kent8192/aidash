use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;
fn a(name: &str) -> Alias {
	Alias::new(name)
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.alter_table(
				Table::alter()
					.table(a("dashboard_sessions"))
					.add_column(
						ColumnDef::new(a("desktop"))
							.boolean()
							.not_null()
							.default(false),
					)
					.add_column(ColumnDef::new(a("desktop_idle_seconds")).big_integer())
					.add_column(ColumnDef::new(a("access_expires_at")).timestamp_with_time_zone())
					.to_owned(),
			)
			.await?;
		manager
			.create_table(
				Table::create()
					.table(a("desktop_handoffs"))
					.col(ColumnDef::new(a("id")).uuid().primary_key())
					.col(ColumnDef::new(a("state")).text().not_null())
					.col(ColumnDef::new(a("challenge")).text().not_null())
					.col(ColumnDef::new(a("redirect_uri")).text().not_null())
					.col(ColumnDef::new(a("origin")).text().not_null())
					.col(ColumnDef::new(a("browser_session_id")).uuid())
					.col(ColumnDef::new(a("code_hash")).binary().unique_key())
					.col(
						ColumnDef::new(a("expires_at"))
							.timestamp_with_time_zone()
							.not_null(),
					)
					.foreign_key(
						ForeignKey::create()
							.from(a("desktop_handoffs"), a("browser_session_id"))
							.to(a("dashboard_sessions"), a("id"))
							.on_delete(ForeignKeyAction::Cascade),
					)
					.to_owned(),
			)
			.await?;
		manager
			.create_index(
				Index::create()
					.name("desktop_handoff_expiry")
					.table(a("desktop_handoffs"))
					.col(a("expires_at"))
					.to_owned(),
			)
			.await?;
		manager
			.create_table(
				Table::create()
					.table(a("desktop_refresh_credentials"))
					.col(ColumnDef::new(a("token_hash")).binary().primary_key())
					.col(ColumnDef::new(a("session_id")).uuid().not_null())
					.col(ColumnDef::new(a("next_hash")).binary())
					.foreign_key(
						ForeignKey::create()
							.from(a("desktop_refresh_credentials"), a("session_id"))
							.to(a("dashboard_sessions"), a("id"))
							.on_delete(ForeignKeyAction::Cascade),
					)
					.to_owned(),
			)
			.await?;
		manager
			.create_index(
				Index::create()
					.name("desktop_refresh_session")
					.table(a("desktop_refresh_credentials"))
					.col(a("session_id"))
					.to_owned(),
			)
			.await?;
		Ok(())
	}
	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		for name in ["desktop_refresh_credentials", "desktop_handoffs"] {
			manager
				.drop_table(Table::drop().table(a(name)).to_owned())
				.await?;
		}
		manager
			.alter_table(
				Table::alter()
					.table(a("dashboard_sessions"))
					.drop_column(a("desktop"))
					.drop_column(a("desktop_idle_seconds"))
					.drop_column(a("access_expires_at"))
					.to_owned(),
			)
			.await?;
		Ok(())
	}
}
