use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if crate::legacy_applied(manager, 3, "1c2dcd4b2242292d8bfdf0aedf0a680b4afc6d3ac1481106ce7b6052a372c695e586f5b16f83bfa92908227edeffe0f7").await? { return Ok(()); }
        // SeaQuery 0.32 has no ALTER TABLE CHECK constraint builder.
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE tasks DROP CONSTRAINT tasks_status_check")
            .await?;
        // SeaQuery 0.32 has no ALTER TABLE CHECK constraint builder.
        manager.get_connection().execute_unprepared("ALTER TABLE tasks ADD CONSTRAINT tasks_status_check CHECK (status IN ('OPEN','CLAIMED','RUNNING','COMPLETED','FAILED','BLOCKED','CANCELLED','ABANDONED'))").await?;
        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        crate::ensure_not_legacy(manager, 3).await?;
        manager.get_connection().execute_unprepared("ALTER TABLE tasks DROP CONSTRAINT tasks_status_check; ALTER TABLE tasks ADD CONSTRAINT tasks_status_check CHECK (status IN ('OPEN','CLAIMED','RUNNING','COMPLETED','FAILED','BLOCKED','CANCELLED'))").await?;
        Ok(())
    }
}
