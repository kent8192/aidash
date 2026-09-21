use super::{LocalStatus, Manifest, Status, Vote, history, participant};
use crate::{Error, Result, federation::Federation};
use chrono::Utc;
use reqwest::Method;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;
use std::time::Duration;
use uuid::Uuid;

pub async fn status(f: &Federation, id: Uuid) -> Result<Status> {
    sqlx::query_as("SELECT * FROM atomic_coordinators WHERE id=$1")
        .bind(id)
        .fetch_optional(&f.store.control_pool)
        .await?
        .ok_or_else(|| Error::NotFound("transaction".into()))
}
pub async fn votes(f: &Federation, id: Uuid) -> Result<Vec<Vote>> {
    Ok(sqlx::query_as(
        "SELECT node_id,phase FROM atomic_votes WHERE transaction_id=$1 ORDER BY node_id",
    )
    .bind(id)
    .fetch_all(&f.store.control_pool)
    .await?)
}
pub async fn submit(f: &Federation, manifest: &Manifest) -> Result<Status> {
    manifest.validate()?;
    if manifest.coordinator != f.config.node_id {
        return Err(Error::Invalid("submit to the named coordinator".into()));
    }
    match status(f, manifest.id).await {
        Ok(existing) => {
            if existing.digest != manifest.digest()? || existing.manifest != json!(manifest) {
                return Err(Error::Conflict(
                    "transaction ID already has another immutable manifest".into(),
                ));
            }
            return Ok(existing);
        }
        Err(Error::NotFound(_)) => {}
        Err(error) => return Err(error),
    }
    let remaining = manifest
        .deadline
        .signed_duration_since(Utc::now())
        .num_seconds();
    if !(1..=3600).contains(&remaining) {
        return Err(Error::Invalid(
            "new transaction deadline must be within the next hour".into(),
        ));
    }
    for node in &manifest.participants {
        if node.node_id != f.config.node_id {
            f.peer(&node.node_id).await?;
            let allowed: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM atomic_peer_trust WHERE node_id=$1 AND enabled)",
            )
            .bind(&node.node_id)
            .fetch_one(&f.store.control_pool)
            .await?;
            if !allowed {
                return Err(Error::Forbidden);
            }
        }
    }
    let mut tx = f.store.control_pool.begin().await?;
    let inserted=sqlx::query("INSERT INTO atomic_coordinators(id,digest,manifest) VALUES($1,$2,$3) ON CONFLICT DO NOTHING").bind(manifest.id).bind(manifest.digest()?).bind(json!(manifest)).execute(&mut *tx).await?.rows_affected();
    let stored: Status = sqlx::query_as("SELECT * FROM atomic_coordinators WHERE id=$1")
        .bind(manifest.id)
        .fetch_one(&mut *tx)
        .await?;
    if stored.digest != manifest.digest()? || stored.manifest != json!(manifest) {
        return Err(Error::Conflict(
            "transaction ID already has another immutable manifest".into(),
        ));
    }
    if inserted == 1 {
        for node in &manifest.participants {
            sqlx::query("INSERT INTO atomic_votes(transaction_id,node_id) VALUES($1,$2)")
                .bind(manifest.id)
                .bind(&node.node_id)
                .execute(&mut *tx)
                .await?;
        }
        history(
            &mut tx,
            manifest.id,
            "coordinator",
            "PENDING",
            "immutable manifest admitted",
        )
        .await?;
    }
    tx.commit().await?;
    f.notify.notify_waiters();
    Ok(stored)
}
pub(crate) async fn remote<T: DeserializeOwned>(
    f: &Federation,
    node: &str,
    method: Method,
    path: &str,
    body: Option<&impl Serialize>,
) -> Result<T> {
    let value = body.map(serde_json::to_value).transpose()?;
    tokio::time::timeout(Duration::from_secs(10), async {
        let response = f.peer_response(node, method, path, value.as_ref()).await?;
        let status = response.status();
        if !status.is_success() {
            return Err(match status {
                reqwest::StatusCode::BAD_REQUEST
                | reqwest::StatusCode::UNPROCESSABLE_ENTITY
                | reqwest::StatusCode::NOT_FOUND
                | reqwest::StatusCode::METHOD_NOT_ALLOWED => Error::Invalid(format!(
                    "transaction participant rejected request: {status}"
                )),
                reqwest::StatusCode::UNAUTHORIZED => Error::Unauthorized,
                reqwest::StatusCode::FORBIDDEN => Error::Forbidden,
                reqwest::StatusCode::CONFLICT => Error::Conflict(
                    "transaction participant rejected its state precondition".into(),
                ),
                reqwest::StatusCode::SERVICE_UNAVAILABLE => Error::TransactionPending,
                _ => Error::External(format!("transaction participant returned {status}")),
            });
        }
        crate::response::json(response, 4_194_304).await
    })
    .await
    .map_err(|_| {
        Error::External(
            "transaction participant response timed out; outcome retained for recovery".into(),
        )
    })?
}
pub(crate) async fn decision(f: &Federation, manifest: &Manifest) -> Result<Status> {
    let proof: Status = if manifest.coordinator == f.config.node_id {
        status(f, manifest.id).await?
    } else {
        remote(
            f,
            &manifest.coordinator,
            Method::GET,
            &format!("/transactions/{}/decision", manifest.id),
            None::<&()>,
        )
        .await?
    };
    if proof.id != manifest.id
        || proof.digest != manifest.digest()?
        || proof.manifest != json!(manifest)
        || proof
            .decision
            .as_deref()
            .is_some_and(|d| !matches!(d, "COMMIT" | "ABORT"))
        || (proof.visible && proof.decision.as_deref() != Some("COMMIT"))
    {
        return Err(Error::Conflict(
            "coordinator decision does not match participant manifest".into(),
        ));
    }
    Ok(proof)
}
async fn send(f: &Federation, manifest: &Manifest, node: &str, phase: &str) -> Result<LocalStatus> {
    if node == f.config.node_id {
        match phase {
            "reserve" => participant::reserve(f, &manifest.coordinator, manifest).await,
            "prepare" => participant::prepare(f, &manifest.coordinator, manifest).await,
            _ => participant::finish(f, &manifest.coordinator, manifest).await,
        }
    } else {
        remote(
            f,
            node,
            Method::POST,
            &format!("/transactions/{phase}"),
            Some(manifest),
        )
        .await
    }
}
async fn record_decision(f: &Federation, id: Uuid, decision: &str, reason: &str) -> Result<()> {
    let mut tx = f.store.control_pool.begin().await?;
    let changed = sqlx::query(
        "UPDATE atomic_coordinators SET decision=$2,last_error=$3 WHERE id=$1 AND decision IS NULL",
    )
    .bind(id)
    .bind(decision)
    .bind((!reason.is_empty()).then_some(reason))
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if changed == 1 {
        history(&mut tx, id, "coordinator", decision, reason).await?;
    }
    tx.commit().await?;
    Ok(())
}
async fn lease(f: &Federation, id: Uuid) -> Result<sqlx::Transaction<'static, sqlx::Postgres>> {
    let mut tx = f.store.control_pool.begin().await?;
    let acquired: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended('atomic:' || $1,0))")
            .bind(id.to_string())
            .fetch_one(&mut *tx)
            .await?;
    if !acquired {
        return Err(Error::TransactionPending);
    }
    Ok(tx)
}
pub async fn abort(f: &Federation, id: Uuid) -> Result<Status> {
    // The conditional decision update arbitrates commit versus abort in SQL.
    // Do not require the recovery lease: it spans peer I/O, and contention must
    // not discard an operator's abort. An in-flight transition cannot overwrite
    // this immutable decision; subsequent recovery finalizes the chosen result.
    record_decision(f, id, "ABORT", "operator requested abort").await?;
    let existing = status(f, id).await?;
    if existing.decision.as_deref() == Some("COMMIT") {
        return Err(Error::Conflict("commit is irrevocable".into()));
    }
    Ok(existing)
}

/// Make one durable protocol transition. Recovery repeats this exact function;
/// it needs no process-local state or ownership inferred from a timeout.
pub async fn advance(f: &Federation, id: Uuid) -> Result<Status> {
    let lease = lease(f, id).await?;
    let result = advance_locked(f, id).await;
    // Await advisory-lock release rather than relying on SQLx's asynchronous
    // rollback-on-drop before acknowledging the transition to its caller.
    lease.commit().await?;
    result
}
async fn advance_locked(f: &Federation, id: Uuid) -> Result<Status> {
    let state = status(f, id).await?;
    if state.complete {
        return Ok(state);
    }
    sqlx::query("UPDATE atomic_coordinators SET updated_at=clock_timestamp() WHERE id=$1")
        .bind(id)
        .execute(&f.store.control_pool)
        .await?;
    let manifest: Manifest = serde_json::from_value(state.manifest.clone())?;
    if state.decision.is_none() && manifest.deadline <= Utc::now() {
        record_decision(f, id, "ABORT", "deadline elapsed before durable decision").await?;
        return status(f, id).await;
    }
    let votes = votes(f, id).await?;
    let selected = if state.decision.is_none() {
        votes
            .iter()
            .find(|v| v.phase == "PENDING")
            .map(|v| (v, "reserve"))
            .or_else(|| {
                votes
                    .iter()
                    .find(|v| v.phase == "RESERVED")
                    .map(|v| (v, "prepare"))
            })
    } else if state.decision.as_deref() == Some("ABORT") {
        votes
            .iter()
            .find(|v| v.phase != "ABORTED")
            .map(|v| (v, "finish"))
    } else if !state.visible {
        votes
            .iter()
            .find(|v| !matches!(v.phase.as_str(), "APPLIED" | "COMMITTED"))
            .map(|v| (v, "finish"))
    } else {
        votes
            .iter()
            .find(|v| v.phase != "COMMITTED")
            .map(|v| (v, "finish"))
    };
    if let Some((vote, operation)) = selected {
        match send(f, &manifest, &vote.node_id, operation).await {
            Ok(result) => {
                if result.id != manifest.id
                    || result.coordinator != manifest.coordinator
                    || result.digest != state.digest
                    || result.manifest != state.manifest
                {
                    return Err(Error::Conflict(
                        "participant acknowledged another manifest".into(),
                    ));
                }
                let expected = match operation {
                    "reserve" => "RESERVED",
                    "prepare" => "PREPARED",
                    _ if state.decision.as_deref() == Some("ABORT") => "ABORTED",
                    _ if state.visible => "COMMITTED",
                    _ => "APPLIED",
                };
                if result.phase != expected {
                    return Err(Error::Conflict(
                        "participant returned an unexpected phase".into(),
                    ));
                }
                sqlx::query(
                    "UPDATE atomic_votes SET phase=$3 WHERE transaction_id=$1 AND node_id=$2",
                )
                .bind(id)
                .bind(&vote.node_id)
                .bind(&result.phase)
                .execute(&f.store.control_pool)
                .await?;
                sqlx::query("UPDATE atomic_coordinators SET last_error=NULL WHERE id=$1")
                    .bind(id)
                    .execute(&f.store.control_pool)
                    .await?;
            }
            Err(error) => {
                if state.decision.is_none()
                    && matches!(
                        error,
                        Error::Conflict(_)
                            | Error::Invalid(_)
                            | Error::NotFound(_)
                            | Error::Forbidden
                            | Error::Unauthorized
                    )
                {
                    record_decision(f, id, "ABORT", &error.to_string()).await?;
                } else {
                    sqlx::query("UPDATE atomic_coordinators SET last_error=$2 WHERE id=$1")
                        .bind(id)
                        .bind(error.to_string())
                        .execute(&f.store.control_pool)
                        .await?;
                }
            }
        }
    } else if state.decision.is_none() {
        if !votes.iter().all(|v| v.phase == "PREPARED") {
            return Err(Error::Conflict(
                "commit requires every prepared vote".into(),
            ));
        }
        record_decision(f, id, "COMMIT", "").await?;
    } else {
        let mut tx = f.store.control_pool.begin().await?;
        if state.decision.as_deref() == Some("COMMIT") && !state.visible {
            sqlx::query("UPDATE atomic_coordinators SET visible=true,last_error=NULL WHERE id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            history(
                &mut tx,
                id,
                "coordinator",
                "VISIBLE",
                "every participant durably applied commit",
            )
            .await?;
        } else {
            sqlx::query("UPDATE atomic_coordinators SET complete=true,last_error=NULL WHERE id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            history(
                &mut tx,
                id,
                "coordinator",
                "COMPLETE",
                "every participant finalized",
            )
            .await?;
        }
        tx.commit().await?;
    }
    status(f, id).await
}
pub async fn recover_once(f: &Federation) -> Result<()> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM atomic_coordinators WHERE NOT complete ORDER BY updated_at,id LIMIT 32",
    )
    .fetch_all(&f.store.control_pool)
    .await?;
    for id in ids {
        if let Err(error) = advance(f, id).await {
            tracing::warn!(%id,%error,"atomic transaction recovery pending");
        }
    }
    Ok(())
}
pub async fn run(f: Federation) -> Result<()> {
    loop {
        if let Err(error) = recover_once(&f).await {
            tracing::warn!(%error,"coordinator recovery failed; decisions retained");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
