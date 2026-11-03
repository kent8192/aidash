// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Standalone public catalog and protected distribution operations.
use crate::{
	Error, Marketplace, Result,
	auth::{self, Session},
	json, store,
};
use aidash_domain::marketplace::distribution::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::Utc;
use reinhardt::http::{Request, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json as value};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
struct VersionMetadata {
	plugin_id: Uuid,
	version: PublishedVersion,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatePublisher {
	name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatePlugin {
	publisher_id: Uuid,
	slug: String,
	name: String,
	description: String,
	price_cents: i64,
	currency: String,
	declared_permissions: Vec<String>,
	license: String,
	origin: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Publish {
	version: String,
	files: BTreeMap<String, String>,
	expected_sha256: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Download {
	connection_id: Uuid,
	owner_account_id: Uuid,
	owner_tenant_id: Option<String>,
	plugin_id: Uuid,
	version: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Enrollment {
	deployment_id: String,
	tenant_id: Option<String>,
	#[serde(default = "enrollment_expiry")]
	expires_at: chrono::DateTime<Utc>,
}
fn enrollment_expiry() -> chrono::DateTime<Utc> {
	Utc::now() + chrono::Duration::minutes(10)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateConnection {
	deployment_id: String,
	enrollment_token: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BillingEvent {
	event_id: String,
	plugin_id: Uuid,
	owner: LibraryOwner,
	expires_at: Option<chrono::DateTime<Utc>>,
	revoked: bool,
}
#[derive(Clone, Serialize, Deserialize)]
struct BillingReceipt {
	fingerprint: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRecall {
	plugin_id: Uuid,
	version: String,
	reason: String,
}

fn input<T: serde::de::DeserializeOwned>(request: &Request) -> Result<T> {
	if request.body().len() > 12 * 1024 * 1024 {
		return Err(Error::Invalid("request exceeds 12 MiB"));
	}
	if request
		.headers
		.get("content-type")
		.and_then(|v| v.to_str().ok())
		.is_none_or(|v| v.split(';').next() != Some("application/json"))
	{
		return Err(Error::Invalid("application/json required"));
	}
	Ok(serde_json::from_slice(request.body())?)
}
fn uuid(value: &str) -> Result<Uuid> {
	value.parse().map_err(|_| Error::NotFound)
}
fn bearer(request: &Request) -> Result<&str> {
	request
		.headers
		.get("authorization")
		.and_then(|v| v.to_str().ok())
		.and_then(|v| v.strip_prefix("Bearer "))
		.filter(|v| v.len() <= 512)
		.ok_or(Error::Unauthorized)
}
fn entitlement_key(owner: &LibraryOwner, plugin: Uuid) -> String {
	match owner {
		LibraryOwner::Account(id) => format!("entitlement:account:{id}:{plugin}"),
		LibraryOwner::Tenant {
			deployment_id,
			tenant_id,
		} => format!(
			"entitlement:tenant:{}:{}:{plugin}",
			auth::hash(deployment_id),
			auth::hash(tenant_id)
		),
	}
}
fn service_auth(request: &Request, secret: &str) -> Result<()> {
	if auth::hash(bearer(request)?) != auth::hash(secret) {
		return Err(Error::Forbidden);
	}
	Ok(())
}

impl Marketplace {
	pub(crate) async fn dispatch(&self, request: Request) -> Result<Response> {
		let path = request.uri.path().trim_end_matches('/');
		let path = if path.is_empty() { "/" } else { path };
		let parts: Vec<_> = path.trim_start_matches('/').split('/').collect();
		let method = request.method.as_str();
		match (method, path) {
			("GET", "/") => return self.assets("index.html"),
			("GET", path) if path.starts_with("/assets/") => return self.assets(&path[1..]),
			("GET", "/healthz") => return json(value!({"service":"marketplace","status":"ok"})),
			("GET", "/auth/login") => return self.login().await,
			("GET", "/auth/callback") => return self.callback(&request).await,
			("GET", "/auth/account") => {
				return Ok(Response::new(http::StatusCode::FOUND)
					.with_header(
						"location",
						&format!(
							"{}/settings/account",
							self.config.issuer.trim_end_matches('/')
						),
					)
					.with_header("cache-control", "no-store"));
			}
			("POST", "/auth/logout") => return self.logout(&request).await,
			("GET", "/api/marketplace/session") => return self.session(&request).await,
			("GET", "/api/marketplace/catalog") => return self.catalog().await,
			("POST", "/internal/billing/events") => return self.billing(&request).await,
			("POST", "/internal/enrollments") => return self.enroll(&request).await,
			("POST", "/internal/recalls") => return self.recall(&request).await,
			("POST", "/api/marketplace/downloads") => {
				return self.download_connection(&request).await;
			}
			("GET", "/api/marketplace/recalls") => {
				self.connection(&request).await?;
				return json(value!({"recalls":self.store.all::<Recall>("recall").await?}));
			}
			_ => {}
		}
		if method == "GET" && parts.len() == 4 && parts[..3] == ["api", "marketplace", "catalog"] {
			let slug = percent_encoding::percent_decode_str(parts[3])
				.decode_utf8()
				.map_err(|_| Error::NotFound)?;
			return self.detail(&slug).await;
		}
		let session = self.authenticate(&request).await?;
		if method != "GET" {
			self.csrf(&request, &session)?;
		}
		match (method, path) {
			("GET", "/api/marketplace/publishers") => {
				return json(
					value!({"publishers":self.store.all::<Publisher>("publisher").await?.into_iter().filter(|p|p.account_id==session.account_id).collect::<Vec<_>>()}),
				);
			}
			("POST", "/api/marketplace/publishers") => {
				return self.create_publisher(&request, &session).await;
			}
			("POST", "/api/marketplace/plugins") => {
				return self.create_plugin(&request, &session).await;
			}
			("GET", "/api/marketplace/library") => return self.library(&session).await,
			("POST", "/api/marketplace/connections") => {
				return self.create_connection(&request, &session).await;
			}
			("GET", "/api/marketplace/connections") => {
				return json(
					value!({"connections":self.store.all::<Connection>("connection").await?.into_iter().filter(|c|c.account_id==session.account_id).map(|c|value!({"id":c.id,"deployment_id":c.deployment_id,"tenant_id":c.tenant_id,"revoked":c.revoked})).collect::<Vec<_>>()}),
				);
			}
			_ => {}
		}
		match (method, parts.as_slice()) {
			("GET", ["api", "marketplace", "publishers", id, "plugins"]) => {
				let publisher = self.publisher(uuid(id)?, session.account_id).await?;
				let plugins = self
					.store
					.all::<Plugin>("plugin")
					.await?
					.into_iter()
					.filter(|p| p.publisher_id == publisher.id)
					.collect::<Vec<_>>();
				json(value!({"plugins":plugins}))
			}
			("POST", ["api", "marketplace", "plugins", id, "versions"]) => {
				self.publish(&request, &session, uuid(id)?).await
			}
			("POST", ["api", "marketplace", "plugins", id, "acquire"]) => {
				self.acquire(&session, uuid(id)?).await
			}
			(
				"GET",
				[
					"api",
					"marketplace",
					"plugins",
					id,
					"versions",
					version,
					"download",
				],
			) => {
				let version = percent_encoding::percent_decode_str(version)
					.decode_utf8()
					.map_err(|_| Error::NotFound)?;
				self.download_personal(&session, uuid(id)?, &version).await
			}
			("POST", ["api", "marketplace", "connections", id, "revoke"]) => {
				self.revoke_connection(&session, uuid(id)?).await
			}
			_ => Err(Error::NotFound),
		}
	}
	async fn catalog(&self) -> Result<Response> {
		let plugins = self
			.store
			.all::<Plugin>("plugin")
			.await?
			.into_iter()
			.filter(|p| p.latest_version.is_some())
			.collect::<Vec<_>>();
		json(value!({"plugins":plugins}))
	}
	async fn detail(&self, slug: &str) -> Result<Response> {
		let plugin = self
			.store
			.all::<Plugin>("plugin")
			.await?
			.into_iter()
			.find(|p| p.slug == slug && p.latest_version.is_some())
			.ok_or(Error::NotFound)?;
		let versions = self.versions(plugin.id).await?;
		json(value!({"plugin":plugin,"versions":versions}))
	}
	async fn versions(&self, id: Uuid) -> Result<Vec<PublishedVersion>> {
		let versions = self.store.all::<VersionMetadata>("version").await?;
		let recalls = self.store.all::<Recall>("recall").await?;
		Ok(versions
			.into_iter()
			.filter(|v| v.plugin_id == id)
			.map(|mut v| {
				if let Some(recall) = recalls.iter().find(|r| {
					r.plugin_id == id
						&& r.version == v.version.version
						&& r.digest == v.version.digest
				}) {
					v.version.recalled = true;
					v.version.recall_reason = Some(recall.reason.clone());
				}
				v.version
			})
			.collect())
	}

	async fn publisher(&self, id: Uuid, account: Uuid) -> Result<Publisher> {
		let p: Publisher = self
			.store
			.get(&format!("publisher:{id}"))
			.await?
			.ok_or(Error::NotFound)?;
		if p.account_id != account {
			return Err(Error::Forbidden);
		}
		Ok(p)
	}
	async fn owned_plugin(&self, id: Uuid, account: Uuid) -> Result<Plugin> {
		let p: Plugin = self
			.store
			.get(&format!("plugin:{id}"))
			.await?
			.ok_or(Error::NotFound)?;
		self.publisher(p.publisher_id, account).await?;
		Ok(p)
	}
	async fn create_publisher(&self, r: &Request, s: &Session) -> Result<Response> {
		let input: CreatePublisher = input(r)?;
		if input.name.trim().is_empty() || input.name.len() > 200 {
			return Err(Error::Invalid("invalid Publisher name"));
		}
		let publisher = Publisher {
			id: Uuid::new_v4(),
			name: input.name,
			account_id: s.account_id,
		};
		self.store
			.connection
			.atomic(async |tx| {
				store::put(
					tx,
					&format!("publisher:{}", publisher.id),
					"publisher",
					&publisher,
				)
				.await
			})
			.await?;
		json(value!({"publisher":publisher}))
	}
	async fn create_plugin(&self, r: &Request, s: &Session) -> Result<Response> {
		let i: CreatePlugin = input(r)?;
		let publisher = self.publisher(i.publisher_id, s.account_id).await?;
		let plugin = Plugin {
			id: Uuid::new_v4(),
			publisher_id: i.publisher_id,
			publisher_name: publisher.name,
			slug: i.slug,
			name: i.name,
			description: i.description,
			price_cents: i.price_cents,
			currency: i.currency,
			declared_permissions: i.declared_permissions,
			license: i.license,
			origin: i.origin,
			latest_version: None,
		};
		plugin.validate().map_err(Error::Invalid)?;
		self.store
			.connection
			.atomic(async |tx| {
				store::lock(tx).await?;
				if store::all::<Plugin>(tx, "plugin")
					.await?
					.iter()
					.any(|p| p.slug == plugin.slug)
				{
					return Err(Error::Conflict);
				}
				store::put(tx, &format!("plugin:{}", plugin.id), "plugin", &plugin).await
			})
			.await?;
		json(value!({"plugin":plugin}))
	}
	async fn publish(&self, r: &Request, s: &Session, id: Uuid) -> Result<Response> {
		let mut plugin = self.owned_plugin(id, s.account_id).await?;
		let i: Publish = input(r)?;
		let decoded = i
			.files
			.iter()
			.map(|(path, content)| {
				Ok(aidash_domain::agent_plugin::PluginFile {
					path: path.clone(),
					bytes: STANDARD
						.decode(content)
						.map_err(|_| Error::Invalid("invalid artifact base64"))?,
				})
			})
			.collect::<Result<Vec<_>>>()?;
		let digest = aidash_domain::agent_plugin::tree_digest(&decoded);
		if i.expected_sha256
			.as_deref()
			.is_some_and(|expected| expected != digest)
		{
			return Err(Error::Invalid("artifact digest mismatch"));
		}
		let artifact = Artifact {
			entitlement_expires_at: None,
			plugin_id: id,
			version: i.version,
			digest,
			files: i.files,
		};
		let size = artifact.validate().map_err(Error::Invalid)?;
		self.store
			.connection
			.atomic(async |tx| {
				store::lock(tx).await?;
				let key = format!("artifact:{id}:{}", artifact.version);
				if store::get::<Artifact>(tx, &key).await?.is_some() {
					return Err(Error::Conflict);
				}
				store::put(tx, &key, "artifact", &artifact).await?;
				store::put(
					tx,
					&format!("version:{id}:{}", artifact.version),
					"version",
					&VersionMetadata {
						plugin_id: id,
						version: PublishedVersion {
							version: artifact.version.clone(),
							digest: artifact.digest.clone(),
							size_bytes: size,
							recalled: false,
							recall_reason: None,
							origin: plugin.origin.clone(),
						},
					},
				)
				.await?;
				plugin.latest_version = Some(artifact.version.clone());
				store::put(tx, &format!("plugin:{id}"), "plugin", &plugin).await
			})
			.await?;
		json(
			value!({"version":PublishedVersion{version:artifact.version,digest:artifact.digest,size_bytes:size,recalled:false,recall_reason:None,origin:plugin.origin}}),
		)
	}
	async fn acquire(&self, s: &Session, id: Uuid) -> Result<Response> {
		let plugin: Plugin = self
			.store
			.get(&format!("plugin:{id}"))
			.await?
			.ok_or(Error::NotFound)?;
		if plugin.price_cents != 0 {
			return Err(Error::Forbidden);
		}
		if plugin.latest_version.is_none() {
			return Err(Error::NotFound);
		}
		let entitlement = Entitlement {
			plugin_id: id,
			owner: LibraryOwner::Account(s.account_id),
			expires_at: None,
			billing_event_id: None,
		};
		self.store
			.connection
			.atomic(async |tx| {
				store::put(
					tx,
					&entitlement_key(&entitlement.owner, id),
					"entitlement",
					&entitlement,
				)
				.await
			})
			.await?;
		json(value!({"acquired":true,"plugin_id":id}))
	}
	async fn library(&self, s: &Session) -> Result<Response> {
		let entitlements = self.store.all::<Entitlement>("entitlement").await?;
		let plugins = self.store.all::<Plugin>("plugin").await?;
		let mut entries = Vec::new();
		for entitlement in entitlements
			.into_iter()
			.filter(|e| e.owner == LibraryOwner::Account(s.account_id) && e.active_at(Utc::now()))
		{
			if let Some(plugin) = plugins.iter().find(|p| p.id == entitlement.plugin_id) {
				entries.push(value!({"plugin_id":plugin.id,"slug":plugin.slug,"name":plugin.name,"versions":self.versions(plugin.id).await?}));
			}
		}
		json(value!({"entries":entries}))
	}
	async fn authorized_artifact(
		&self,
		owner: &LibraryOwner,
		id: Uuid,
		version: &str,
	) -> Result<Artifact> {
		self.store
			.connection
			.atomic(async |tx| {
				store::lock(tx).await?;
				let e: Entitlement = store::get(tx, &entitlement_key(owner, id))
					.await?
					.ok_or(Error::Forbidden)?;
				if !e.active_at(Utc::now()) {
					return Err(Error::Forbidden);
				}
				let mut artifact: Artifact = store::get(tx, &format!("artifact:{id}:{version}"))
					.await?
					.ok_or(Error::NotFound)?;
				if store::get::<Recall>(tx, &format!("recall:{id}:{version}"))
					.await?
					.is_some()
				{
					return Err(Error::Forbidden);
				}
				artifact.entitlement_expires_at = e.expires_at;
				artifact.validate().map_err(Error::Invalid)?;
				Ok(artifact)
			})
			.await
	}
	async fn download_personal(&self, s: &Session, id: Uuid, version: &str) -> Result<Response> {
		let artifact = self
			.authorized_artifact(&LibraryOwner::Account(s.account_id), id, version)
			.await?;
		let filename = format!("attachment; filename=\"{id}-{}.json\"", artifact.version);
		Ok(json(artifact)?.with_header("content-disposition", &filename))
	}

	async fn connection(&self, r: &Request) -> Result<Connection> {
		let digest = auth::hash(bearer(r)?);
		let connection = self
			.store
			.all::<Connection>("connection")
			.await?
			.into_iter()
			.find(|c| c.credential_digest == digest && !c.revoked)
			.ok_or(Error::Unauthorized)?;
		let response = self
			.http
			.post(format!(
				"{}/internal/accounts/check",
				self.config.issuer.trim_end_matches('/')
			))
			.basic_auth(
				&self.config.resource_client_id,
				Some(&self.config.resource_secret),
			)
			.json(&value!({"account_id":connection.account_id}))
			.send()
			.await
			.map_err(|_| Error::Unavailable)?;
		if !response.status().is_success() {
			return Err(Error::Unavailable);
		}
		let status: Value = response.json().await.map_err(|_| Error::Unavailable)?;
		if status["active"] != true {
			return Err(Error::Unauthorized);
		}
		Ok(connection)
	}
	async fn download_connection(&self, r: &Request) -> Result<Response> {
		let i: Download = input(r)?;
		let c = self.connection(r).await?;
		if c.id != i.connection_id || c.account_id != i.owner_account_id {
			return Err(Error::Forbidden);
		}
		let owner = match i.owner_tenant_id {
			Some(id) => {
				if c.tenant_id.as_deref() != Some(id.as_str()) {
					return Err(Error::Forbidden);
				}
				LibraryOwner::Tenant {
					deployment_id: c.deployment_id.clone(),
					tenant_id: id,
				}
			}
			None => LibraryOwner::Account(c.account_id),
		};
		json(
			self.authorized_artifact(&owner, i.plugin_id, &i.version)
				.await?,
		)
	}
	async fn create_connection(&self, r: &Request, s: &Session) -> Result<Response> {
		let i: CreateConnection = input(r)?;
		if i.deployment_id.is_empty() || i.deployment_id.len() > 200 {
			return Err(Error::Invalid("invalid Deployment ID"));
		}
		let credential = auth::secret();
		let connection = self
			.store
			.connection
			.atomic(async |tx| {
				store::lock(tx).await?;
				let key = format!("enrollment:{}", auth::hash(&i.enrollment_token));
				let enrollment: Enrollment = store::get(tx, &key).await?.ok_or(Error::Forbidden)?;
				if enrollment.expires_at <= Utc::now()
					|| enrollment.deployment_id != i.deployment_id
				{
					return Err(Error::Forbidden);
				}
				store::remove(tx, &key).await?;
				let connection = Connection {
					id: Uuid::new_v4(),
					account_id: s.account_id,
					deployment_id: i.deployment_id,
					tenant_id: enrollment.tenant_id,
					credential_digest: auth::hash(&credential),
					revoked: false,
				};
				store::put(
					tx,
					&format!("connection:{}", connection.id),
					"connection",
					&connection,
				)
				.await?;
				Ok::<_, Error>(connection)
			})
			.await?;
		json(
			value!({"connection":{"id":connection.id,"deployment_id":connection.deployment_id,"tenant_id":connection.tenant_id},"credential":credential}),
		)
	}
	async fn revoke_connection(&self, s: &Session, id: Uuid) -> Result<Response> {
		self.store
			.connection
			.atomic(async |tx| {
				store::lock(tx).await?;
				let key = format!("connection:{id}");
				let mut c: Connection = store::get(tx, &key).await?.ok_or(Error::NotFound)?;
				if c.account_id != s.account_id {
					return Err(Error::Forbidden);
				}
				c.revoked = true;
				store::put(tx, &key, "connection", &c).await
			})
			.await?;
		json(value!({"revoked":true}))
	}
	async fn enroll(&self, r: &Request) -> Result<Response> {
		service_auth(r, &self.config.enrollment_secret)?;
		let mut enrollment: Enrollment = input(r)?;
		enrollment.expires_at = enrollment_expiry();
		if enrollment.deployment_id.is_empty()
			|| enrollment.deployment_id.len() > 200
			|| enrollment
				.tenant_id
				.as_ref()
				.is_some_and(|t| t.is_empty() || t.len() > 200)
		{
			return Err(Error::Invalid("invalid enrollment scope"));
		}
		let token = auth::secret();
		self.store
			.connection
			.atomic(async |tx| {
				store::put(
					tx,
					&format!("enrollment:{}", auth::hash(&token)),
					"enrollment",
					&enrollment,
				)
				.await
			})
			.await?;
		json(value!({"enrollment_token":token}))
	}
	async fn billing(&self, r: &Request) -> Result<Response> {
		service_auth(r, &self.config.billing_secret)?;
		let i: BillingEvent = input(r)?;
		if i.event_id.is_empty() || i.event_id.len() > 200 {
			return Err(Error::Invalid("invalid billing event ID"));
		}
		let receipt_key = format!("billing:{}", auth::hash(&i.event_id));
		let fingerprint =
			auth::hash(std::str::from_utf8(r.body()).map_err(|_| Error::Invalid("invalid event"))?);
		self.store
			.connection
			.atomic(async |tx| {
				store::lock(tx).await?;
				if let Some(receipt) = store::get::<BillingReceipt>(tx, &receipt_key).await? {
					if receipt.fingerprint != fingerprint {
						return Err(Error::Conflict);
					}
					return Ok(());
				}
				if store::get::<Plugin>(tx, &format!("plugin:{}", i.plugin_id))
					.await?
					.is_none()
				{
					return Err(Error::NotFound);
				}
				let key = entitlement_key(&i.owner, i.plugin_id);
				if i.revoked {
					store::remove(tx, &key).await?
				} else {
					store::put(
						tx,
						&key,
						"entitlement",
						&Entitlement {
							plugin_id: i.plugin_id,
							owner: i.owner,
							expires_at: i.expires_at,
							billing_event_id: Some(i.event_id),
						},
					)
					.await?
				}
				store::put(
					tx,
					&receipt_key,
					"billing_receipt",
					&BillingReceipt { fingerprint },
				)
				.await
			})
			.await?;
		json(value!({"accepted":true}))
	}
	async fn recall(&self, r: &Request) -> Result<Response> {
		service_auth(r, &self.config.recall_secret)?;
		let i: CreateRecall = input(r)?;
		let id = i.plugin_id;
		let version = i.version.as_str();
		if i.reason.trim().is_empty() || i.reason.len() > 2000 {
			return Err(Error::Invalid("invalid Recall reason"));
		}
		let artifact: Artifact = self
			.store
			.get(&format!("artifact:{id}:{version}"))
			.await?
			.ok_or(Error::NotFound)?;
		let recall = Recall {
			plugin_id: id,
			version: version.to_owned(),
			digest: artifact.digest,
			reason: i.reason,
			recalled_at: Utc::now(),
		};
		self.store
			.connection
			.atomic(async |tx| {
				store::lock(tx).await?;
				let key = format!("recall:{id}:{version}");
				if store::get::<Recall>(tx, &key).await?.is_none() {
					store::put(tx, &key, "recall", &recall).await?
				}
				Ok::<_, Error>(())
			})
			.await?;
		json(value!({"recalled":true}))
	}
}
