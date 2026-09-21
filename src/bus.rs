use crate::{Error, Result, domain::Event, federation::Federation};
use async_nats::jetstream::{
    self,
    consumer::{AckPolicy, pull},
    stream,
};
use futures_util::StreamExt;
use serde_json::Value;
use std::time::Duration;

#[derive(Clone)]
pub struct EventBus {
    pub context: jetstream::Context,
    pub stream_name: String,
    pub subject: String,
}
impl EventBus {
    /// Broker connection and consumer recovery never gate HTTP or worker startup.
    pub async fn run(f: Federation) -> Result<()> {
        loop {
            let result = async {
                let bus = tokio::time::timeout(
                    Duration::from_secs(15),
                    Self::connect(&f.config.nats_url, &f.config.node_id),
                )
                .await
                .map_err(|_| Error::External("event bus connection timed out".into()))??;
                tokio::select! {
                    result = bus.publisher(f.clone()) => result,
                    result = bus.consumer(f.clone()) => result,
                }
            }
            .await;
            if let Err(error) = result {
                tracing::warn!(%error, "event bus unavailable; durable state retained for retry");
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    pub async fn connect(url: &str, node_id: &str) -> Result<Self> {
        let client = async_nats::connect(url)
            .await
            .map_err(|e| Error::External(e.to_string()))?;
        let context = jetstream::new(client);
        let suffix = node_id
            .strip_prefix("aidash://")
            .ok_or_else(|| Error::Invalid("invalid node id".into()))?;
        let stream_name = format!("AIDASH_{}", suffix.replace('-', "_"));
        let subject = format!("aidash.{suffix}.events");
        context
            .get_or_create_stream(stream::Config {
                name: stream_name.clone(),
                subjects: vec![subject.clone()],
                storage: stream::StorageType::File,
                duplicate_window: Duration::from_secs(120),
                ..Default::default()
            })
            .await
            .map_err(|e| Error::External(e.to_string()))?;
        Ok(Self {
            context,
            stream_name,
            subject,
        })
    }
    pub async fn publish_once(&self, f: &Federation) -> Result<usize> {
        let _visibility = crate::transactions::gate::ReadLease::begin(&f.store).await?;
        let events: Vec<Event> = sqlx::query_as(&sea_orm::sea_query::Query::update().table(sea_orm::sea_query::Alias::new("events")).value(sea_orm::sea_query::Alias::new("next_attempt_at"), sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP + INTERVAL '30 SECONDS'")).and_where(sea_orm::sea_query::Expr::cust("id IN (SELECT id FROM events WHERE published_at IS NULL AND next_attempt_at <= CURRENT_TIMESTAMP ORDER BY next_attempt_at, sequence LIMIT 100 FOR UPDATE SKIP LOCKED)")).returning(sea_orm::sea_query::Query::returning().exprs([sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("sequence"))), sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("id"))), sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("node_id"))), sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("workspace_id"))), sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("kind"))), sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("data"))), sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("created_at")))])).to_string(sea_orm::sea_query::PostgresQueryBuilder))
            .fetch_all(&f.store.pool).await?;
        let mut attempts = futures_util::stream::iter(events.into_iter().map(|event| async move {
            let mut headers = async_nats::HeaderMap::new();
            let message_id = event.id.to_string();
            let header_len = b"NATS/1.0\r\nNats-Msg-Id: \r\n\r\n".len() + message_id.len();
            headers.insert("Nats-Msg-Id", message_id);
            let result = async {
                let mut envelope = event.cloud_event();
                let mut payload = envelope.to_string();
                let limit = self.context.client().server_info().max_payload;
                // Keep the durable payload in PostgreSQL and publish a reference
                // when broker limits cannot accommodate the complete event.
                if payload.len().saturating_add(header_len) > limit {
                    envelope
                        .as_object_mut()
                        .expect("CloudEvent object")
                        .remove("data");
                    envelope["dataref"] =
                        serde_json::json!(format!("/api/events?after={}", event.sequence - 1));
                    payload = envelope.to_string();
                }
                // Even reference envelopes must fit before HPUB: oversize frames
                // disconnect the shared connection and disrupt healthy events.
                if payload.len().saturating_add(header_len) > limit {
                    return Err(Error::External(format!(
                        "event exceeds NATS max_payload {limit}"
                    )));
                }
                self.context
                    .publish_with_headers(self.subject.clone(), headers, payload.into())
                    .await
                    .map_err(|error| Error::External(error.to_string()))?
                    .await
                    .map_err(|error| Error::External(error.to_string()))?;
                Result::Ok(())
            }
            .await;
            match result {
                Ok(()) => {
                    sqlx::query(
                        &sea_orm::sea_query::Query::update()
                            .table(sea_orm::sea_query::Alias::new("events"))
                            .value(
                                sea_orm::sea_query::Alias::new("published_at"),
                                sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP"),
                            )
                            .value(
                                sea_orm::sea_query::Alias::new("publish_error"),
                                sea_orm::sea_query::Expr::cust("NULL"),
                            )
                            .and_where(sea_orm::sea_query::Expr::cust("id = $1"))
                            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
                    )
                    .bind(event.id)
                    .execute(&f.store.pool)
                    .await?;
                    Ok(1)
                }
                Err(error) => {
                    // Retain the event and a visible error; later rows keep flowing.
                    sqlx::query(
                        &sea_orm::sea_query::Query::update()
                            .table(sea_orm::sea_query::Alias::new("events"))
                            .value(
                                sea_orm::sea_query::Alias::new("publish_error"),
                                sea_orm::sea_query::Expr::cust("$2"),
                            )
                            .value(
                                sea_orm::sea_query::Alias::new("next_attempt_at"),
                                sea_orm::sea_query::Expr::cust(
                                    "CURRENT_TIMESTAMP + INTERVAL '30 SECONDS'",
                                ),
                            )
                            .and_where(sea_orm::sea_query::Expr::cust("id = $1"))
                            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
                    )
                    .bind(event.id)
                    .bind(error.to_string())
                    .execute(&f.store.pool)
                    .await?;
                    tracing::warn!(event_id=%event.id, %error, "outbox event retained for retry");
                    Ok::<usize, Error>(0)
                }
            }
        }))
        .buffer_unordered(8);
        let mut published = 0;
        while let Some(result) = attempts.next().await {
            published += result?;
        }
        Ok(published)
    }
    pub async fn publisher(&self, f: Federation) -> Result<()> {
        loop {
            if let Err(e) = self.publish_once(&f).await {
                tracing::warn!(error=%e,"outbox publish failed; retained for retry");
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    pub async fn consumer(&self, f: Federation) -> Result<()> {
        let stream = self
            .context
            .get_stream(&self.stream_name)
            .await
            .map_err(|e| Error::External(e.to_string()))?;
        let consumer = stream
            .get_or_create_consumer(
                "execution",
                pull::Config {
                    durable_name: Some("execution".into()),
                    ack_policy: AckPolicy::Explicit,
                    filter_subject: self.subject.clone(),
                    ack_wait: Duration::from_secs(30),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| Error::External(e.to_string()))?;
        loop {
            let mut messages = consumer
                .messages()
                .await
                .map_err(|e| Error::External(e.to_string()))?;
            while let Some(message) = messages.next().await {
                let message = message.map_err(|e| Error::External(e.to_string()))?;
                let id = serde_json::from_slice::<Value>(&message.payload)
                    .ok()
                    .and_then(|event| {
                        event["id"]
                            .as_str()
                            .and_then(|id| id.parse::<uuid::Uuid>().ok())
                    });
                let Some(id) = id else {
                    message
                        .ack_with(jetstream::AckKind::Term)
                        .await
                        .map_err(|e| Error::External(e.to_string()))?;
                    tracing::warn!("discarded malformed broker event");
                    continue;
                };
                let mut tx = f.store.pool.begin().await?;
                let inserted = sqlx::query(
                    &sea_orm::sea_query::Query::insert()
                        .into_table(sea_orm::sea_query::Alias::new("inbox"))
                        .columns([sea_orm::sea_query::Alias::new("event_id")])
                        .values_panic([sea_orm::sea_query::Expr::cust("$1")])
                        .on_conflict(
                            sea_orm::sea_query::OnConflict::new()
                                .do_nothing()
                                .to_owned(),
                        )
                        .to_string(sea_orm::sea_query::PostgresQueryBuilder),
                )
                .bind(id)
                .execute(&mut *tx)
                .await?
                .rows_affected();
                tx.commit().await?;
                // The durable task/run tables are the consumer's work queue.
                // Restart scanning recovers a crash between commit and wakeup.
                if inserted > 0 {
                    f.notify.notify_waiters();
                }
                message
                    .ack()
                    .await
                    .map_err(|e| Error::External(e.to_string()))?;
            }
        }
    }
}
