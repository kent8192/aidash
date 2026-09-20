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
        let events:Vec<Event>=sqlx::query_as("SELECT sequence,id,node_id,workspace_id,kind,data,created_at FROM events WHERE published_at IS NULL ORDER BY sequence LIMIT 100").fetch_all(&f.store.pool).await?;
        for event in &events {
            let mut headers = async_nats::HeaderMap::new();
            headers.insert("Nats-Msg-Id", event.id.to_string());
            self.context
                .publish_with_headers(
                    self.subject.clone(),
                    headers,
                    event.cloud_event().to_string().into(),
                )
                .await
                .map_err(|e| Error::External(e.to_string()))?
                .await
                .map_err(|e| Error::External(e.to_string()))?;
            sqlx::query("UPDATE events SET published_at=now() WHERE id=$1")
                .bind(event.id)
                .execute(&f.store.pool)
                .await?;
        }
        Ok(events.len())
    }
    pub async fn publisher(&self, f: Federation) -> Result<()> {
        loop {
            if let Err(e) = self.publish_once(&f).await {
                tracing::warn!(error=%e,"outbox publish failed; retained for retry");
            }
            if let Err(e) = f.retry_deliveries().await {
                tracing::warn!(error=%e,"delegation retry failed");
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
                let event: Value = serde_json::from_slice(&message.payload)?;
                let id = event["id"]
                    .as_str()
                    .ok_or_else(|| Error::Invalid("event id missing".into()))?
                    .parse::<uuid::Uuid>()
                    .map_err(|_| Error::Invalid("invalid event id".into()))?;
                let mut tx = f.store.pool.begin().await?;
                let inserted =
                    sqlx::query("INSERT INTO inbox(event_id) VALUES($1) ON CONFLICT DO NOTHING")
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
