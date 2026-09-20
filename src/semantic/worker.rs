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
    let ids:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM semantic_entries WHERE NOT deleted AND next_attempt<=clock_timestamp() ORDER BY next_attempt,id LIMIT 32").fetch_all(&store.pool).await?;
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
        "SELECT workspace_id,authority FROM semantic_entries WHERE id=$1 AND NOT deleted",
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
        let Some(mut entry)=sqlx::query_as::<_,Entry>("SELECT * FROM semantic_entries WHERE id=$1 AND NOT deleted AND next_attempt<=clock_timestamp() FOR UPDATE SKIP LOCKED").bind(id).fetch_optional(&mut **lease.tx()).await? else {return Ok(false)};
        let current:Value=sqlx::query_scalar("SELECT authority FROM semantic_entries WHERE id=$1").bind(id).fetch_one(&mut **lease.tx()).await?;
        if current!=authority {return Ok(false);}
        let spec=index.configuration()?;
        let source:Source=serde_json::from_value(entry.source.clone())?;
        let permitted=workspace_allowed && lease.permits(&entry,"semantic.write").await? && lease.permits(&entry,"semantic.read").await?;
        let text=if permitted && spec.enabled {lease.source(workspace,&source).await?} else {None};
        let Some(text)=text else {
            sqlx::query("UPDATE semantic_entries SET state='REVOKED',last_error=$2,next_attempt=clock_timestamp()+interval '30 seconds' WHERE id=$1").bind(id).bind(if spec.enabled {"source authority revoked or source removed"} else {"semantic index disabled"}).execute(&mut **lease.tx()).await?;
            sqlx::query("UPDATE semantic_points SET retired=true,next_attempt=clock_timestamp() WHERE entry_id=$1 AND NOT retired").bind(id).execute(&mut **lease.tx()).await?;
            if entry.state!="REVOKED" {service::history(lease.tx(),workspace,Some(id),entry.revision,"REVOKED","indexing authority or source unavailable").await?;}
            return Ok(true);
        };
        let digest=service::content_digest(&text);
        let old:Option<(Option<String>,bool)>=sqlx::query_as("SELECT content_digest,retired FROM semantic_points WHERE id=$1").bind(entry.point_id).fetch_optional(&mut **lease.tx()).await?;
        if entry.state=="READY" && old.as_ref().is_some_and(|(d,retired)|d.as_deref()==Some(&digest)&&!*retired)
            && backend::present(&store.semantic_client, &spec.vector,&index.collection,&[entry.point_id]).await.unwrap_or(false) {
            sqlx::query("UPDATE semantic_entries SET next_attempt=clock_timestamp()+interval '30 seconds' WHERE id=$1").bind(id).execute(&mut **lease.tx()).await?;
            return Ok(true);
        }
        if old.as_ref().is_some_and(|(d,retired)|*retired || d.as_deref().is_some_and(|d|d!=digest)) {
            // Never overwrite a point whose old revision may still appear in
            // delayed query/upsert responses. Its tombstone remains durable.
            entry=sqlx::query_as("UPDATE semantic_entries SET revision=revision+1,point_id=$2,state='PENDING',attempts=0,last_error=NULL WHERE id=$1 RETURNING *").bind(id).bind(Uuid::new_v4()).fetch_one(&mut **lease.tx()).await?;
            service::schedule_point(lease.tx(),&entry,&index.collection).await?;
            service::history(lease.tx(),workspace,Some(id),entry.revision,"PENDING","linked source changed or authority restored").await?;
            // Commit the new cleanup identity before any external request.
            return Ok(true);
        }
        let result=async {
            service::validate_text(&text,spec.max_input_bytes)?;
            let vector=backend::embed(&store.semantic_client, &spec.embedding,&text).await?;
            backend::ensure_collection(&store.semantic_client, &spec.vector,&index.collection,spec.embedding.dimensions).await?;
            backend::upsert(&store.semantic_client, &spec.vector,&index.collection,entry.point_id,&vector,json!({"entry_id":id,"revision":entry.revision,"index_revision":index.revision,"workspace_id":workspace,"tenant":index.tenant})).await
        }.await;
        match result {
            Ok(())=> {
                sqlx::query("UPDATE semantic_points SET content_digest=$2 WHERE id=$1 AND NOT retired").bind(entry.point_id).bind(digest).execute(&mut **lease.tx()).await?;
                sqlx::query("UPDATE semantic_entries SET state='READY',attempts=0,last_error=NULL,updated_at=clock_timestamp(),next_attempt=clock_timestamp()+interval '30 seconds' WHERE id=$1").bind(id).execute(&mut **lease.tx()).await?;
                service::history(lease.tx(),workspace,Some(id),entry.revision,"READY","vector write acknowledged").await?;
            }
            Err(_)=> {
                let attempts=entry.attempts.saturating_add(1);
                let delay=2_i64.pow(attempts.min(8) as u32).min(300);
                sqlx::query("UPDATE semantic_entries SET state='ERROR',attempts=$2,last_error='embedding or vector backend unavailable, invalid, or source too large',updated_at=clock_timestamp(),next_attempt=clock_timestamp()+make_interval(secs=>$3) WHERE id=$1").bind(id).bind(attempts).bind(delay as f64).execute(&mut **lease.tx()).await?;
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
    let entry:Option<Entry>=sqlx::query_as("UPDATE semantic_entries SET state='REVOKED',last_error='indexing credential or policy revoked',next_attempt=clock_timestamp()+interval '30 seconds' WHERE id=$1 AND authority=$2 AND NOT deleted RETURNING *").bind(id).bind(authority).fetch_optional(&mut *tx).await?;
    if let Some(entry) = entry {
        sqlx::query("UPDATE semantic_points SET retired=true,next_attempt=clock_timestamp() WHERE entry_id=$1 AND NOT retired").bind(id).execute(&mut *tx).await?;
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
    let points:Vec<Uuid>=sqlx::query_scalar("SELECT p.id FROM semantic_points p JOIN semantic_collections c ON c.collection=p.collection WHERE p.retired AND NOT c.retired AND p.next_attempt<=clock_timestamp() ORDER BY p.next_attempt,p.id LIMIT 32").fetch_all(&store.pool).await?;
    let collections:Vec<String>=sqlx::query_scalar("SELECT collection FROM semantic_collections WHERE retired AND next_attempt<=clock_timestamp() ORDER BY next_attempt,collection LIMIT 16").fetch_all(&store.pool).await?;
    drop(visibility);
    for id in points {
        let _visibility = crate::transactions::gate::ReadLease::begin(store).await?;
        let mut tx = store.pool.begin().await?;
        let row:Option<(String,Value)>=sqlx::query_as("SELECT p.collection,c.vector FROM semantic_points p JOIN semantic_collections c ON c.collection=p.collection WHERE p.id=$1 AND p.retired AND p.next_attempt<=clock_timestamp() FOR UPDATE OF p SKIP LOCKED").bind(id).fetch_optional(&mut *tx).await?;
        if let Some((collection, config)) = row {
            let config: VectorConfig = serde_json::from_value(config)?;
            let failure = backend::delete_point(&store.semantic_client, &config, &collection, id)
                .await
                .is_err()
                .then_some("vector deletion failed; retry scheduled");
            sqlx::query("UPDATE semantic_points SET last_error=$2,cleaned_at=CASE WHEN $2::text IS NULL THEN clock_timestamp() ELSE cleaned_at END,next_attempt=clock_timestamp()+interval '30 seconds' WHERE id=$1").bind(id).bind(failure).execute(&mut *tx).await?;
        }
        tx.commit().await?;
    }
    for collection in collections {
        let _visibility = crate::transactions::gate::ReadLease::begin(store).await?;
        let mut tx = store.pool.begin().await?;
        let config:Option<Value>=sqlx::query_scalar("SELECT vector FROM semantic_collections WHERE collection=$1 AND retired AND next_attempt<=clock_timestamp() FOR UPDATE SKIP LOCKED").bind(&collection).fetch_optional(&mut *tx).await?;
        if let Some(config) = config {
            let config: VectorConfig = serde_json::from_value(config)?;
            let failure = backend::delete_collection(&store.semantic_client, &config, &collection)
                .await
                .is_err()
                .then_some("collection deletion failed; retry scheduled");
            sqlx::query("UPDATE semantic_collections SET last_error=$2,cleaned_at=CASE WHEN $2::text IS NULL THEN clock_timestamp() ELSE cleaned_at END,next_attempt=clock_timestamp()+interval '30 seconds' WHERE collection=$1").bind(collection).bind(failure).execute(&mut *tx).await?;
        }
        tx.commit().await?;
    }
    Ok(())
}
