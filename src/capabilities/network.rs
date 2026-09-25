//! Outbound HTTP is executed here, never by passing credentials or a general
//! proxy into untrusted code. DNS answers are checked and pinned per redirect.
use super::{
	approvals,
	contracts::*,
	records::{self, Record},
	sessions,
};
use crate::{
	Error, Result,
	authorization::{access::Access, identity::SubjectIdentity},
	store::Store,
};
use chrono::{Duration, Utc};
use sea_orm::sea_query::{Alias, Expr, Order, PostgresQueryBuilder, Query};
use serde_json::json;
use std::{
	net::{IpAddr, SocketAddr},
	time::Duration as StdDuration,
};
use uuid::Uuid;
fn public_ip(ip: IpAddr) -> bool {
	match ip {
		IpAddr::V4(ip) => {
			let [a, b, c, _] = ip.octets();
			!ip.is_unspecified()
				&& !ip.is_loopback()
				&& !ip.is_private()
				&& !ip.is_link_local()
				&& !ip.is_broadcast()
				&& !ip.is_multicast()
				&& a != 0 && a < 224
				&& !(a == 100 && (64..128).contains(&b))
				&& !(a == 192 && b == 0 && (c == 0 || c == 2))
				&& !(a == 198 && (b == 18 || b == 19 || b == 51 && c == 100))
				&& !(a == 203 && b == 0 && c == 113)
		}
		IpAddr::V6(ip) => {
			let s = ip.segments();
			// Only native global unicast; reject transition/tunnel and special
			// protocol/documentation ranges as well as mapped IPv4 addresses.
			(s[0] & 0xe000) == 0x2000
				&& !(s[0] == 0x2001 && (s[1] < 0x200 || s[1] == 0xdb8))
				&& s[0] != 0x2002
				&& !(s[0] == 0x3fff && (s[1] & 0xf000) == 0)
		}
	}
}
async fn fetch(store: &Store, record: &Record) -> Result<(u16, Vec<u8>, String)> {
	let mut url = record.data["url"]
		.as_str()
		.ok_or(Error::Forbidden)?
		.to_owned();
	for _ in 0..5 {
		let (parsed, origin) = approvals::permitted_origin(store, &url)?;
		if !record.data["targets"]
			.as_array()
			.is_some_and(|a| a.contains(&json!(origin)))
		{
			return Err(Error::Forbidden);
		}
		let host = parsed.host_str().ok_or(Error::Forbidden)?;
		let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host, 443)).await?.collect();
		if addresses.is_empty()
			|| addresses.len() > 32
			|| addresses.iter().any(|a| !public_ip(a.ip()))
		{
			return Err(Error::Forbidden);
		}
		let client = reqwest::Client::builder()
			.no_proxy()
			.user_agent(concat!("Aidash/", env!("CARGO_PKG_VERSION")))
			.redirect(reqwest::redirect::Policy::none())
			.timeout(StdDuration::from_secs(10))
			.resolve_to_addrs(host, &addresses)
			.build()?;
		let mut response = client
			.get(parsed)
			.header("Accept", "text/plain, application/json;q=0.9, */*;q=0.1")
			.send()
			.await?;
		if response.status().is_redirection() {
			let next = response
				.headers()
				.get(reqwest::header::LOCATION)
				.and_then(|h| h.to_str().ok())
				.ok_or_else(|| Error::Invalid("INVALID_REDIRECT".into()))?;
			url = response
				.url()
				.join(next)
				.map_err(|_| Error::Invalid("INVALID_REDIRECT".into()))?
				.to_string();
			continue;
		}
		let status = response.status().as_u16();
		let mut bytes = vec![];
		while let Some(chunk) = response.chunk().await? {
			if bytes.len() + chunk.len() > 1 << 20 {
				return Err(Error::Invalid("OUTBOUND_RESPONSE_LIMIT".into()));
			}
			bytes.extend_from_slice(&chunk);
		}
		return Ok((status, bytes, url));
	}
	Err(Error::Invalid("OUTBOUND_REDIRECT_LIMIT".into()))
}
fn identity(record: &Record) -> Result<SubjectIdentity> {
	Ok(SubjectIdentity {
		credential_id: serde_json::from_value(record.data["credential_id"].clone())?,
		tenant: record.tenant.clone(),
		subject: record.owner.clone(),
	})
}
async fn withdrawn(store: &Store, record: &Record) -> Result<()> {
	let identity = identity(record)?;
	loop {
		let mut access = Access::begin(store, &identity).await?;
		let current = records::get(&mut access, record.id, "outbound").await?;
		let checked = approvals::authorize(store, &mut access, &current)
			.await
			.map(|_| ());
		access.finish(checked).await?;
		tokio::time::sleep(StdDuration::from_millis(200)).await;
	}
}
async fn fail(store: &Store, id: Uuid, message: &str) -> Result<()> {
	sqlx::query(&Query::update().table(Alias::new("core_records"))
        .value(Alias::new("state"),Expr::val("uncertain"))
        .value(Alias::new("data"),Expr::cust("data || $2::jsonb"))
        .value(Alias::new("revision"),Expr::col(Alias::new("revision")).add(1))
        .and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
        .and_where(Expr::col(Alias::new("state")).is_in(["approved","attempted"]))
        .to_string(PostgresQueryBuilder)).bind(id).bind(json!({"error":{"code":message,"message":"Outbound execution stopped; earlier effects may have occurred.","retryable":false}})).execute(&store.pool).await?;
	Ok(())
}
async fn drive(store: &Store, id: Uuid) -> Result<()> {
	let snapshot: Record = sqlx::query_as(
		&sessions::select("core_records")
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.fetch_one(&store.pool)
	.await?;
	if snapshot.state == "attempted" {
		return fail(store, id, "OUTBOUND_EFFECT_UNCERTAIN").await;
	}
	let identity = identity(&snapshot)?;
	let mut access = Access::begin(store, &identity).await?;
	let accepted = async {
		let mut record = records::get(&mut access, id, "outbound").await?;
		if record.state != "approved" {
			return Ok(None);
		}
		record.state = "attempted".into();
		record.data["attempted_at"] = json!(Utc::now());
		record.data["attempt_deadline"] = json!(Utc::now() + Duration::seconds(90));
		approvals::authorize(store, &mut access, &record).await?;
		records::update(&mut access, &mut record).await?;
		Ok(Some(record))
	}
	.await;
	let Some(record) = access.finish(accepted).await? else {
		return Ok(());
	};
	let outcome = tokio::select! {
		result=tokio::time::timeout(StdDuration::from_secs(60),fetch(store,&record))=>result.map_err(|_|Error::External("outbound timeout".into()))?,
		_=withdrawn(store,&record)=>Err(Error::Forbidden),
	};
	let (http_status, bytes, final_url) = match outcome {
		Ok(result) => result,
		Err(_) => return fail(store, id, "OUTBOUND_STOPPED").await,
	};
	let mut access = Access::begin(store, &identity).await?;
	let result = async {
		let mut record = records::get(&mut access, id, "outbound").await?;
		let run = approvals::authorize(store, &mut access, &record).await?;
		let (file_id, digest) = store
			.capabilities
			.put(&mut access, record.area_id, "network_output", &bytes)
			.await?;
		let entry = FileEntry {
			file_id,
			path: format!("outbound-{id}.bin"),
			digest,
			size: bytes.len() as u64,
			media_type: "application/octet-stream".into(),
			scope: FileScope::Working,
			provenance: json!({"kind":"outbound","operation_id":id}),
		};
		record.state = "completed".into();
		record.data["output_file"] = json!(entry);
		record.data["http_status"] = json!(http_status);
		record.data["final_url"] = json!(final_url);
		records::update(&mut access, &mut record).await?;
		store
			.event(
				&mut access.tx,
				Some(run.workspace_id),
				"capability.outbound_completed",
				json!({"operation_id":id,"run_id":run.id,"http_status":http_status}),
			)
			.await?;
		Ok(())
	}
	.await;
	access.finish(result).await
}
pub(crate) async fn run(
	store: Store,
	mut stopping: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
	loop {
		if *stopping.borrow() {
			return Ok(());
		}
		let ids:Vec<Uuid>=sqlx::query_scalar(&Query::select().column(Alias::new("id")).from(Alias::new("core_records"))
            .and_where(Expr::col(Alias::new("kind")).eq("outbound"))
            .and_where(Expr::cust("state = 'approved' OR (state = 'attempted' AND (data->>'attempt_deadline')::timestamptz < CURRENT_TIMESTAMP)"))
            .order_by(Alias::new("id"),Order::Asc).limit(8).to_string(PostgresQueryBuilder)).fetch_all(&store.pool).await?;
		use futures_util::{StreamExt, stream};
		let runner_store = &store;
		stream::iter(ids)
			.for_each_concurrent(8, |id| async move {
				if let Err(error) = Box::pin(drive(runner_store, id)).await {
					tracing::warn!(%id,%error,"outbound operation stopped");
					let _ = fail(runner_store, id, "AUTHORITY_WITHDRAWN").await;
				}
			})
			.await;
		tokio::select! {_=stopping.changed()=>{},_=tokio::time::sleep(StdDuration::from_millis(300))=>{}}
	}
}
