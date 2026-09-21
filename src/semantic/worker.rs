//! Restart-safe indexing and repeated cleanup of retired physical identities.
use super::{
    service::{self, Lease},
    *,
};
use crate::{federation::Federation, store::Store};
use serde_json::json;
use std::time::Duration;

pub async fn run(f: Federation) -> Result<()> {
    loop {
        match sweep(&f.store).await {
            Ok(_) | Err(Error::TransactionPending) => {}
            Err(error) => tracing::warn!(%error,"semantic indexing sweep failed"),
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
pub async fn sweep(store: &Store) -> Result<usize> {
    let visibility = crate::transactions::gate::ReadLease::begin(store).await?;
    let ids: Vec<Uuid> = sqlx::query_scalar(
        &sea_orm::sea_query::Query::select()
            .expr(sea_orm::sea_query::SimpleExpr::from(
                sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("id")),
            ))
            .from(sea_orm::sea_query::Alias::new("semantic_entries"))
            .and_where(sea_orm::sea_query::Expr::cust(
                "NOT deleted AND next_attempt <= CLOCK_TIMESTAMP()",
            ))
            .order_by_expr(
                sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
                    sea_orm::sea_query::Alias::new("next_attempt"),
                )),
                sea_orm::sea_query::Order::Asc,
            )
            .order_by_expr(
                sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
                    sea_orm::sea_query::Alias::new("id"),
                )),
                sea_orm::sea_query::Order::Asc,
            )
            .limit(32)
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .fetch_all(&store.pool)
    .await?;
    drop(visibility);
    let mut processed = 0;
    for id in ids {
        let _visibility = crate::transactions::gate::ReadLease::begin(store).await?;
        if process(store, id).await? {
            processed += 1;
        }
    }
    cleanup(store).await?;
    Ok(processed)
}
async fn process(store: &Store, id: Uuid) -> Result<bool> {
    let Some((workspace, authority)) = sqlx::query_as::<_, (Uuid, Value)>(
        &sea_orm::sea_query::Query::select()
            .expr(sea_orm::sea_query::SimpleExpr::from(
                sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("workspace_id")),
            ))
            .expr(sea_orm::sea_query::SimpleExpr::from(
                sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("authority")),
            ))
            .from(sea_orm::sea_query::Alias::new("semantic_entries"))
            .and_where(sea_orm::sea_query::Expr::cust("id = $1 AND NOT deleted"))
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .bind(id)
    .fetch_optional(&store.pool)
    .await?
    else {
        return Ok(false);
    };
    let mut lease = match Lease::restore(store, authority.clone()).await {
        Ok(lease) => lease,
        Err(Error::Forbidden | Error::Unauthorized | Error::NotFound(_)) => {
            revoke(store, workspace, id, &authority).await?;
            return Ok(true);
        }
        Err(error) => return Err(error),
    };
    lease.durable();
    let result=async {
        let workspace_allowed=match lease.workspace(workspace,"semantic.write").await {
            Ok(())=>true,
            Err(Error::Forbidden)=>false,
            Err(error)=>return Err(error),
        };
        let index=service::index(lease.tx(),workspace,false).await?;
        let Some(mut entry)=sqlx::query_as::<_,Entry>(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk))).from(sea_orm::sea_query::Alias::new("semantic_entries")).and_where(sea_orm::sea_query::Expr::cust("id = $1 AND NOT deleted AND next_attempt <= CLOCK_TIMESTAMP()")).lock_with_behavior(sea_orm::sea_query::LockType::Update, sea_orm::sea_query::LockBehavior::SkipLocked).to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(id).fetch_optional(&mut **lease.tx()).await? else {return Ok(false)};
        let current:Value=sqlx::query_scalar(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("authority")))).from(sea_orm::sea_query::Alias::new("semantic_entries")).and_where(sea_orm::sea_query::Expr::cust("id = $1")).to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(id).fetch_one(&mut **lease.tx()).await?;
        if current!=authority {return Ok(false);}
        let spec=index.configuration()?;
        let source:Source=serde_json::from_value(entry.source.clone())?;
        let permitted=workspace_allowed && lease.permits(&entry,"semantic.write").await? && lease.permits(&entry,"semantic.read").await?;
        let text=if permitted && spec.enabled {lease.source(workspace,&source).await?} else {None};
        let Some(text)=text else {
            sqlx::query(&sea_orm::sea_query::Query::update().table(sea_orm::sea_query::Alias::new("semantic_entries")).value(sea_orm::sea_query::Alias::new("state"), sea_orm::sea_query::Expr::cust("'REVOKED'")).value(sea_orm::sea_query::Alias::new("last_error"), sea_orm::sea_query::Expr::cust("$2")).value(sea_orm::sea_query::Alias::new("next_attempt"), sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP() + INTERVAL '30 SECONDS'")).and_where(sea_orm::sea_query::Expr::cust("id = $1")).to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(id).bind(if spec.enabled {"source authority revoked or source removed"} else {"semantic index disabled"}).execute(&mut **lease.tx()).await?;
            sqlx::query(&sea_orm::sea_query::Query::update().table(sea_orm::sea_query::Alias::new("semantic_points")).value(sea_orm::sea_query::Alias::new("retired"), sea_orm::sea_query::Expr::cust("TRUE")).value(sea_orm::sea_query::Alias::new("next_attempt"), sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP()")).and_where(sea_orm::sea_query::Expr::cust("entry_id = $1 AND NOT retired")).to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(id).execute(&mut **lease.tx()).await?;
            if entry.state!="REVOKED" {service::history(lease.tx(),workspace,Some(id),entry.revision,"REVOKED","indexing authority or source unavailable").await?;}
            return Ok(true);
        };
        let digest=service::content_digest(&text);
        let old:Option<(Option<String>,bool)>=sqlx::query_as(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("content_digest")))).expr(sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("retired")))).from(sea_orm::sea_query::Alias::new("semantic_points")).and_where(sea_orm::sea_query::Expr::cust("id = $1")).to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(entry.point_id).fetch_optional(&mut **lease.tx()).await?;
        if entry.state=="READY" && old.as_ref().is_some_and(|(d,retired)|d.as_deref()==Some(&digest)&&!*retired)
            && backend::present(&store.semantic_client, &spec.vector,&index.collection,&[entry.point_id]).await.unwrap_or(false) {
            sqlx::query(&sea_orm::sea_query::Query::update().table(sea_orm::sea_query::Alias::new("semantic_entries")).value(sea_orm::sea_query::Alias::new("next_attempt"), sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP() + INTERVAL '30 SECONDS'")).and_where(sea_orm::sea_query::Expr::cust("id = $1")).to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(id).execute(&mut **lease.tx()).await?;
            return Ok(true);
        }
        if old.as_ref().is_some_and(|(d,retired)|*retired || d.as_deref().is_some_and(|d|d!=digest)) {
            // Never overwrite a point whose old revision may still appear in
            // delayed query/upsert responses. Its tombstone remains durable.
            entry=sqlx::query_as(&sea_orm::sea_query::Query::update().table(sea_orm::sea_query::Alias::new("semantic_entries")).value(sea_orm::sea_query::Alias::new("revision"), sea_orm::sea_query::Expr::cust("revision + 1")).value(sea_orm::sea_query::Alias::new("point_id"), sea_orm::sea_query::Expr::cust("$2")).value(sea_orm::sea_query::Alias::new("state"), sea_orm::sea_query::Expr::cust("'PENDING'")).value(sea_orm::sea_query::Alias::new("attempts"), sea_orm::sea_query::Expr::cust("0")).value(sea_orm::sea_query::Alias::new("last_error"), sea_orm::sea_query::Expr::cust("NULL")).and_where(sea_orm::sea_query::Expr::cust("id = $1")).returning_all().to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(id).bind(Uuid::new_v4()).fetch_one(&mut **lease.tx()).await?;
            service::schedule_point(lease.tx(),&entry,&index.collection).await?;
            service::history(lease.tx(),workspace,Some(id),entry.revision,"PENDING","linked source changed or authority restored").await?;
            // Commit the new cleanup identity before any external request.
            return Ok(true);
        }
        let result=async {
            service::validate_text(&text,spec.max_input_bytes)?;
            backend::ensure_collection(&store.semantic_client, &spec.vector,&index.collection,spec.embedding.dimensions).await?;
            let vector=service::embed(store, &mut lease, workspace, &spec.embedding, &text, crate::generation::embedding::Origin::Index(id)).await?;
            backend::upsert(&store.semantic_client, &spec.vector,&index.collection,entry.point_id,&vector,json!({"entry_id":id,"revision":entry.revision,"index_revision":index.revision,"workspace_id":workspace,"tenant":index.tenant})).await
        }.await;
        match result {
            Ok(())=> {
                sqlx::query(&sea_orm::sea_query::Query::update().table(sea_orm::sea_query::Alias::new("semantic_points")).value(sea_orm::sea_query::Alias::new("content_digest"), sea_orm::sea_query::Expr::cust("$2")).and_where(sea_orm::sea_query::Expr::cust("id = $1 AND NOT retired")).to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(entry.point_id).bind(digest).execute(&mut **lease.tx()).await?;
                sqlx::query(&sea_orm::sea_query::Query::update().table(sea_orm::sea_query::Alias::new("semantic_entries")).value(sea_orm::sea_query::Alias::new("state"), sea_orm::sea_query::Expr::cust("'READY'")).value(sea_orm::sea_query::Alias::new("attempts"), sea_orm::sea_query::Expr::cust("0")).value(sea_orm::sea_query::Alias::new("last_error"), sea_orm::sea_query::Expr::cust("NULL")).value(sea_orm::sea_query::Alias::new("updated_at"), sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP()")).value(sea_orm::sea_query::Alias::new("next_attempt"), sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP() + INTERVAL '30 SECONDS'")).and_where(sea_orm::sea_query::Expr::cust("id = $1")).to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(id).execute(&mut **lease.tx()).await?;
                service::history(lease.tx(),workspace,Some(id),entry.revision,"READY","vector write acknowledged").await?;
            }
            Err(_)=> {
                let attempts=entry.attempts.saturating_add(1);
                let delay=2_i64.pow(attempts.min(8) as u32).min(300);
                sqlx::query(&sea_orm::sea_query::Query::update().table(sea_orm::sea_query::Alias::new("semantic_entries")).value(sea_orm::sea_query::Alias::new("state"), sea_orm::sea_query::Expr::cust("'ERROR'")).value(sea_orm::sea_query::Alias::new("attempts"), sea_orm::sea_query::Expr::cust("$2")).value(sea_orm::sea_query::Alias::new("last_error"), sea_orm::sea_query::Expr::cust("'embedding or vector backend unavailable, invalid, or source too large'")).value(sea_orm::sea_query::Alias::new("updated_at"), sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP()")).value(sea_orm::sea_query::Alias::new("next_attempt"), sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP() + MAKE_INTERVAL(secs => $3)")).and_where(sea_orm::sea_query::Expr::cust("id = $1")).to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(id).bind(attempts).bind(delay as f64).execute(&mut **lease.tx()).await?;
                service::history(lease.tx(),workspace,Some(id),entry.revision,"ERROR","indexing failed; durable retry scheduled").await?;
            }
        }
        Ok(true)
    }.await;
    lease.finish(result).await
}
async fn revoke(store: &Store, workspace: Uuid, id: Uuid, authority: &Value) -> Result<()> {
    let mut tx = store.pool.begin().await?;
    service::index(&mut tx, workspace, false).await?;
    let entry: Option<Entry> = sqlx::query_as(
        &sea_orm::sea_query::Query::update()
            .table(sea_orm::sea_query::Alias::new("semantic_entries"))
            .value(
                sea_orm::sea_query::Alias::new("state"),
                sea_orm::sea_query::Expr::cust("'REVOKED'"),
            )
            .value(
                sea_orm::sea_query::Alias::new("last_error"),
                sea_orm::sea_query::Expr::cust("'indexing credential or policy revoked'"),
            )
            .value(
                sea_orm::sea_query::Alias::new("next_attempt"),
                sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP() + INTERVAL '30 SECONDS'"),
            )
            .and_where(sea_orm::sea_query::Expr::cust(
                "id = $1 AND authority = $2 AND NOT deleted",
            ))
            .returning_all()
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .bind(id)
    .bind(authority)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(entry) = entry {
        sqlx::query(
            &sea_orm::sea_query::Query::update()
                .table(sea_orm::sea_query::Alias::new("semantic_points"))
                .value(
                    sea_orm::sea_query::Alias::new("retired"),
                    sea_orm::sea_query::Expr::cust("TRUE"),
                )
                .value(
                    sea_orm::sea_query::Alias::new("next_attempt"),
                    sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP()"),
                )
                .and_where(sea_orm::sea_query::Expr::cust(
                    "entry_id = $1 AND NOT retired",
                ))
                .to_string(sea_orm::sea_query::PostgresQueryBuilder),
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        service::history(
            &mut tx,
            workspace,
            Some(id),
            entry.revision,
            "REVOKED",
            "indexing authority unavailable",
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}
async fn cleanup(store: &Store) -> Result<()> {
    // Keep tombstones after acknowledgement: timed-out requests can arrive
    // late. Release the node visibility lease between independent effects so
    // a failing backend does not block atomic transaction admission for a batch.
    let visibility = crate::transactions::gate::ReadLease::begin(store).await?;
    let points:Vec<Uuid>=sqlx::query_scalar(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((sea_orm::sea_query::Alias::new("p"), sea_orm::sea_query::Alias::new("id"))))).from_as(sea_orm::sea_query::Alias::new("semantic_points"), sea_orm::sea_query::Alias::new("p")).join_as(sea_orm::sea_query::JoinType::InnerJoin, sea_orm::sea_query::Alias::new("semantic_collections"), sea_orm::sea_query::Alias::new("c"), sea_orm::sea_query::Expr::cust("c.collection = p.collection")).and_where(sea_orm::sea_query::Expr::cust("p.retired AND p.cleaned_at IS NULL AND NOT c.retired AND p.next_attempt <= CLOCK_TIMESTAMP()")).order_by_expr(sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((sea_orm::sea_query::Alias::new("p"), sea_orm::sea_query::Alias::new("next_attempt")))), sea_orm::sea_query::Order::Asc).order_by_expr(sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((sea_orm::sea_query::Alias::new("p"), sea_orm::sea_query::Alias::new("id")))), sea_orm::sea_query::Order::Asc).limit(32).to_string(sea_orm::sea_query::PostgresQueryBuilder)).fetch_all(&store.pool).await?;
    let collections: Vec<String> = sqlx::query_scalar(
        &sea_orm::sea_query::Query::select()
            .expr(sea_orm::sea_query::SimpleExpr::from(
                sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("collection")),
            ))
            .from(sea_orm::sea_query::Alias::new("semantic_collections"))
            .and_where(sea_orm::sea_query::Expr::cust(
                "retired AND cleaned_at IS NULL AND next_attempt <= CLOCK_TIMESTAMP()",
            ))
            .order_by_expr(
                sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
                    sea_orm::sea_query::Alias::new("next_attempt"),
                )),
                sea_orm::sea_query::Order::Asc,
            )
            .order_by_expr(
                sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
                    sea_orm::sea_query::Alias::new("collection"),
                )),
                sea_orm::sea_query::Order::Asc,
            )
            .limit(16)
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .fetch_all(&store.pool)
    .await?;
    drop(visibility);
    for id in points {
        let _visibility = crate::transactions::gate::ReadLease::begin(store).await?;
        let mut tx = store.pool.begin().await?;
        let row:Option<(String,Value)>=sqlx::query_as(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((sea_orm::sea_query::Alias::new("p"), sea_orm::sea_query::Alias::new("collection"))))).expr(sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((sea_orm::sea_query::Alias::new("c"), sea_orm::sea_query::Alias::new("vector"))))).from_as(sea_orm::sea_query::Alias::new("semantic_points"), sea_orm::sea_query::Alias::new("p")).join_as(sea_orm::sea_query::JoinType::InnerJoin, sea_orm::sea_query::Alias::new("semantic_collections"), sea_orm::sea_query::Alias::new("c"), sea_orm::sea_query::Expr::cust("c.collection = p.collection")).and_where(sea_orm::sea_query::Expr::cust("p.id = $1 AND p.retired AND p.cleaned_at IS NULL AND p.next_attempt <= CLOCK_TIMESTAMP()")).lock_with_tables_behavior(sea_orm::sea_query::LockType::Update, [sea_orm::sea_query::Alias::new("p")], sea_orm::sea_query::LockBehavior::SkipLocked).to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(id).fetch_optional(&mut *tx).await?;
        if let Some((collection, config)) = row {
            let config: VectorConfig = serde_json::from_value(config)?;
            let failure = backend::delete_point(&store.semantic_client, &config, &collection, id)
                .await
                .is_err()
                .then_some("vector deletion failed; retry scheduled");
            sqlx::query(&sea_orm::sea_query::Query::update().table(sea_orm::sea_query::Alias::new("semantic_points")).value(sea_orm::sea_query::Alias::new("last_error"), sea_orm::sea_query::Expr::cust("$2")).value(sea_orm::sea_query::Alias::new("cleaned_at"), sea_orm::sea_query::Expr::cust("CASE WHEN CAST($2 AS TEXT) IS NULL THEN CLOCK_TIMESTAMP() ELSE cleaned_at END")).value(sea_orm::sea_query::Alias::new("next_attempt"), sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP() + INTERVAL '30 SECONDS'")).and_where(sea_orm::sea_query::Expr::cust("id = $1")).to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(id).bind(failure).execute(&mut *tx).await?;
        }
        tx.commit().await?;
    }
    for collection in collections {
        let _visibility = crate::transactions::gate::ReadLease::begin(store).await?;
        let mut tx = store.pool.begin().await?;
        let config:Option<Value>=sqlx::query_scalar(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("vector")))).from(sea_orm::sea_query::Alias::new("semantic_collections")).and_where(sea_orm::sea_query::Expr::cust("collection = $1 AND retired AND cleaned_at IS NULL AND next_attempt <= CLOCK_TIMESTAMP()")).lock_with_behavior(sea_orm::sea_query::LockType::Update, sea_orm::sea_query::LockBehavior::SkipLocked).to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(&collection).fetch_optional(&mut *tx).await?;
        if let Some(config) = config {
            let config: VectorConfig = serde_json::from_value(config)?;
            let failure = backend::delete_collection(&store.semantic_client, &config, &collection)
                .await
                .is_err()
                .then_some("collection deletion failed; retry scheduled");
            sqlx::query(&sea_orm::sea_query::Query::update().table(sea_orm::sea_query::Alias::new("semantic_collections")).value(sea_orm::sea_query::Alias::new("last_error"), sea_orm::sea_query::Expr::cust("$2")).value(sea_orm::sea_query::Alias::new("cleaned_at"), sea_orm::sea_query::Expr::cust("CASE WHEN CAST($2 AS TEXT) IS NULL THEN CLOCK_TIMESTAMP() ELSE cleaned_at END")).value(sea_orm::sea_query::Alias::new("next_attempt"), sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP() + INTERVAL '30 SECONDS'")).and_where(sea_orm::sea_query::Expr::cust("collection = $1")).to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(collection).bind(failure).execute(&mut *tx).await?;
        }
        tx.commit().await?;
    }
    Ok(())
}
