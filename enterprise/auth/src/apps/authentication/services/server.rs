// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Composition of shared identity lifecycle, upstream GCIP and Reinhardt's issuer.
use crate::{Error, Result, config::Config, store::Store};
use aidash_application::{
	accounts::Service as Accounts,
	ports::{accounts::Scope, authorization::dashboard::AccountStatus as UpstreamStatus},
};
use aidash_domain::accounts::*;
use aidash_gcp::gcip::{self, TokenVerifier};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{Duration, Utc};
use reinhardt::auth::{
	AuthIdentity, UserRepository,
	core::{Argon2Hasher, PasswordHasher},
	oauth2_server::{
		ClientKind, ClientRegistration, OAuthRateLimiter, OAuthServer, OAuthServerConfig,
		OAuthServerStore, PostgresOAuthStore, RefreshTokenPolicy, ResourceRegistration,
		SharedOAuthRateLimiter,
	},
	oidc_op::{
		OidcAccountStatus, OidcConfig, OidcProvider, OidcStateStore, PostgresOidcStore,
		RsaPemKeyRing,
	},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path, sync::Arc};
use uuid::Uuid;

pub(crate) fn digest(value: &str) -> String {
	use base64::engine::general_purpose::URL_SAFE_NO_PAD;
	URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
}
pub(crate) fn secret() -> String {
	format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}
pub fn read_secret(path: &Path) -> Result<String> {
	let value = std::fs::read_to_string(path).map_err(|_| Error::Configuration)?;
	let value = value.trim_end_matches(['\r', '\n']);
	if value.is_empty() {
		return Err(Error::Configuration);
	}
	Ok(value.into())
}

#[derive(Clone)]
pub struct Authentication {
	pub config: Arc<Config>,
	pub store: Store,
	pub oauth: Arc<OAuthServer>,
	pub oidc: Arc<OidcProvider>,
	pub(crate) verifier: Arc<TokenVerifier>,
	pub(crate) upstream: Arc<dyn UpstreamStatus>,
	resource_secrets: Arc<BTreeMap<String, String>>,
	pub(crate) encryption_key: Arc<[u8; 32]>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Flow {
	pub pending: String,
	pub client_id: String,
	pub csrf: String,
	pub tenant: String,
	pub started_at: chrono::DateTime<Utc>,
	pub expires_at: chrono::DateTime<Utc>,
	pub proof: Option<FreshProof>,
	pub target_proof: Option<FreshProof>,
	pub consumed: bool,
	pub purpose: String,
	pub account: Option<Uuid>,
	pub unlink_identity: Option<Uuid>,
}
impl Authentication {
	pub async fn connect(config: Config) -> Result<Store> {
		config.validate().map_err(|_| Error::Configuration)?;
		let url = read_secret(&config.database_url_file)?;
		let options: sqlx::postgres::PgConnectOptions =
			url.parse().map_err(|_| Error::Configuration)?;
		let options = options.options([("search_path", "aidash_auth,pg_catalog")]);
		let pool = sqlx::postgres::PgPoolOptions::new()
			.max_connections(24)
			.connect_with(options)
			.await
			.map_err(|_| Error::Unavailable)?;
		let store = Store::from_pool(pool);
		let (sql, values) = reinhardt::query::Query::select()
			.expr_as(
				reinhardt::query::SimpleExpr::FunctionCall(
					reinhardt::query::Alias::new("current_schema").into_iden(),
					vec![],
				),
				reinhardt::query::Alias::new("schema"),
			)
			.build(reinhardt::query::PostgresQueryBuilder);
		let row = store
			.connection
			.fetch_one(&sql, reinhardt::db::orm::execution::convert_values(values))
			.await
			.map_err(|_| Error::Unavailable)?;
		if row.data.get("schema")
			!= Some(&reinhardt::db::backends::QueryValue::String(
				"aidash_auth".into(),
			)) {
			return Err(Error::Configuration);
		}
		Ok(store)
	}
	pub async fn new(config: Config, store: Store) -> Result<Self> {
		config.validate().map_err(|_| Error::Configuration)?;
		// A serving process must not own the schema or forge migration history.
		// PostgreSQL's two-argument privilege functions use the current role.
		let function = |name: &str, args: Vec<reinhardt::query::SimpleExpr>| {
			reinhardt::query::SimpleExpr::FunctionCall(
				reinhardt::query::Alias::new(name).into_iden(),
				args,
			)
		};
		let (sql, values) = reinhardt::query::Query::select()
			.expr_as(
				function(
					"has_schema_privilege",
					vec![
						reinhardt::query::Expr::value("aidash_auth").into(),
						reinhardt::query::Expr::value("CREATE").into(),
					],
				),
				reinhardt::query::Alias::new("schema_owner"),
			)
			.expr_as(
				function(
					"has_table_privilege",
					vec![
						reinhardt::query::Expr::value("aidash_auth.reinhardt_migrations").into(),
						reinhardt::query::Expr::value(
							"INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER",
						)
						.into(),
					],
				),
				reinhardt::query::Alias::new("ledger_write"),
			)
			.build(reinhardt::query::PostgresQueryBuilder);
		let row = store
			.connection
			.fetch_one(&sql, reinhardt::db::orm::execution::convert_values(values))
			.await
			.map_err(|_| Error::Configuration)?;
		if row.data.get("schema_owner") != Some(&reinhardt::db::backends::QueryValue::Bool(false))
			|| row.data.get("ledger_write")
				!= Some(&reinhardt::db::backends::QueryValue::Bool(false))
		{
			return Err(Error::Configuration);
		}
		let expected = crate::migrations::migrations();
		let applied =
			reinhardt::db::migrations::DatabaseMigrationRecorder::new(store.connection.clone())
				.get_applied_migrations_if_present()
				.await
				.map_err(|_| Error::Unavailable)?;
		if expected.iter().any(|m| {
			!applied
				.iter()
				.any(|a| a.app == m.app_label && a.name == m.name)
		}) || applied.len() != expected.len()
		{
			return Err(Error::Configuration);
		}
		let (oauth_config, oidc_config) = protocol_config(&config)?;
		let users = Arc::new(AccountsAdapter(store.clone()));
		let limiter = Arc::new(SharedLimiter(store.clone()));
		let oauth_store = PostgresOAuthStore::new(store.pool.clone());
		let oauth = Arc::new(if config.loopback_development {
			OAuthServer::for_development(
				oauth_config,
				Arc::new(oauth_store),
				users.clone(),
				limiter,
			)?
		} else {
			OAuthServer::for_production(oauth_config, oauth_store, users.clone(), limiter)?
		});
		let signer = Arc::new(RsaPemKeyRing::new());
		signer
			.add_private_key_pem(
				&config.signing_key_id,
				&read_secret(&config.signing_key_file)?,
			)
			.map_err(|_| Error::Configuration)?;
		let oidc_store = PostgresOidcStore::new(store.pool.clone());
		let oidc = Arc::new(if config.loopback_development {
			OidcProvider::for_development(
				oidc_config,
				oauth.clone(),
				Arc::new(oidc_store),
				users,
				signer,
			)?
		} else {
			OidcProvider::for_production(oidc_config, oauth.clone(), oidc_store, users, signer)
				.await?
		});
		let http = reqwest::Client::builder()
			.redirect(reqwest::redirect::Policy::none())
			.timeout(std::time::Duration::from_secs(10))
			.build()
			.map_err(|_| Error::Configuration)?;
		let verifier = Arc::new(TokenVerifier {
			project: config.gcip_project.clone(),
			keys: Arc::new(aidash_integrations::jwks::HttpSigningKeys::new(
				http.clone(),
				gcip::JWKS_URL.into(),
			)),
		});
		let upstream = Arc::new(gcip::AccountLookup {
			client: http,
			project: config.gcip_project.clone(),
			endpoint: "https://identitytoolkit.googleapis.com/".into(),
			credentials: Arc::new(gcip::ApplicationDefaultCredentials::default()),
		});
		let mut resource_secrets = BTreeMap::new();
		for client in &config.clients {
			resource_secrets.insert(
				format!("{}-resource", client.id),
				digest(&read_secret(&client.introspection_secret_file)?),
			);
		}
		let encryption_key: [u8; 32] = STANDARD
			.decode(read_secret(&config.token_encryption_key_file)?)
			.map_err(|_| Error::Configuration)?
			.try_into()
			.map_err(|_| Error::Configuration)?;
		Ok(Self {
			config: Arc::new(config),
			store,
			oauth,
			oidc,
			verifier,
			upstream,
			resource_secrets: Arc::new(resource_secrets),
			encryption_key: Arc::new(encryption_key),
		})
	}
	/// Run only with the schema owner's credential, after applying this service's
	/// migrations. Runtime startup never registers clients or changes credentials.
	pub async fn provision(config: &Config, store: &Store) -> Result<()> {
		let oauth = PostgresOAuthStore::new(store.pool.clone());
		let audience = format!("{}/oauth/userinfo", config.issuer);
		oauth
			.put_resource(ResourceRegistration {
				audience: audience.clone(),
				resource_id: "userinfo".into(),
				secret_hash: hash_secret(secret()).await?,
				enabled: true,
			})
			.await
			.map_err(|_| Error::Unavailable)?;
		for client in &config.clients {
			let mut registration = ClientRegistration::new(&client.id, ClientKind::Confidential);
			registration.secret_hash = Some(hash_secret(read_secret(&client.secret_file)?).await?);
			registration.authorization_code = true;
			registration.refresh_token = true;
			registration.oidc_enabled = true;
			registration.redirect_uris = vec![client.redirect_uri.clone()];
			registration.scopes = vec!["openid".into()];
			registration.default_scopes = vec!["openid".into()];
			registration.audiences = vec![audience.clone()];
			registration.default_audience = Some(audience.clone());
			oauth
				.put_client(registration)
				.await
				.map_err(|_| Error::Unavailable)?;
		}
		let signer = RsaPemKeyRing::new();
		let public = signer
			.add_private_key_pem(
				&config.signing_key_id,
				&read_secret(&config.signing_key_file)?,
			)
			.map_err(|_| Error::Configuration)?;
		let keys = PostgresOidcStore::new(store.pool.clone());
		if let Some(active) = keys.active_key().await.map_err(|_| Error::Unavailable)?
			&& active.public.kid == public.kid
		{
			// Migration retries retain the active key and its activation time. A
			// changed private key must use a new ID and the normal rotation path.
			return if active.public == public && !active.compromised {
				Ok(())
			} else {
				Err(Error::Configuration)
			};
		}
		keys.rotate_key(public, Utc::now().timestamp(), 960)
			.await
			.map_err(|_| Error::Unavailable)?;
		Ok(())
	}
	pub(crate) async fn refresh_identity(&self, id: Uuid) -> Result<()> {
		let mut tx = self.store.scope().await?;
		let Some(identity) = tx.identity(id).await? else {
			return Ok(());
		};
		if identity.status == IdentityStatus::Disabled || !identity.needs_status_refresh(Utc::now())
		{
			return Ok(());
		}
		drop(tx);
		let observed = self
			.upstream
			.lookup(&identity.issuer, &identity.subject)
			.await;
		if let Ok(status) = observed {
			let mut tx = self.store.scope().await?;
			let mut latest = tx.identity(id).await?.ok_or(Error::Unauthorized)?;
			// A concurrent explicit disable cannot be overwritten by upstream data.
			if latest.status == IdentityStatus::Enabled {
				if status.disabled {
					latest.disable_epoch = latest
						.disable_epoch
						.checked_add(1)
						.ok_or(Error::Unavailable)?;
					latest.status = IdentityStatus::Disabled;
					tx.revoke_identity_sessions(id, Utc::now()).await?;
				} else {
					latest.last_valid_at = Some(Utc::now());
					tx.observe_source(id, status.valid_since).await?;
				}
				tx.save_identity(&latest).await?;
				Box::new(tx).commit().await?;
			}
		}
		// A failed lookup has only the domain's remaining fifteen-minute grace;
		// there is no positive-result cache when this authority is unreachable.
		Ok(())
	}
	pub async fn check_session(
		&self,
		id: Uuid,
	) -> Result<(Validity, Option<ProductSession>, Option<ExternalIdentity>)> {
		let mut tx = self.store.scope().await?;
		let session = tx.session(id).await?;
		drop(tx);
		let Some(session) = session else {
			return Ok((Validity::Denied(Denial::SessionInvalid), None, None));
		};
		self.refresh_identity(session.identity_id).await?;
		let validity = Accounts::new(self.store.clone())
			.check_session(id, Utc::now())
			.await?;
		let mut tx = self.store.scope().await?;
		let source = tx.identity(session.identity_id).await?;
		Ok((validity, Some(session), source))
	}
	pub async fn check_origin(
		&self,
		source: Uuid,
		expected_account: Option<Uuid>,
		expected_epoch: Option<u64>,
		expected_source_epoch: Option<u64>,
		bind_current: bool,
	) -> Result<(Validity, Option<ExternalIdentity>, Option<u64>)> {
		self.refresh_identity(source).await?;
		let mut tx = self.store.scope().await?;
		let Some(identity) = tx.identity(source).await? else {
			return Ok((Validity::Denied(Denial::ProvenanceMismatch), None, None));
		};
		let account = match identity.first_account {
			Some(id) => tx.account(id).await?,
			None => None,
		};
		let epoch = account.as_ref().map(|account| account.suspension_epoch);
		let origin = DurableOrigin {
			identity_id: source,
			account_id: expected_account,
			suspension_epoch: if bind_current { epoch } else { expected_epoch },
			source_disable_epoch: if bind_current {
				Some(identity.disable_epoch)
			} else {
				expected_source_epoch
			},
		};
		let result = durable_validity(&origin, account.as_ref(), &identity, Utc::now());
		Ok((result, Some(identity), epoch))
	}
	pub(crate) fn resource_client(&self, id: &str, raw: &str) -> Result<String> {
		use subtle::ConstantTimeEq;
		let expected = self.resource_secrets.get(id).ok_or(Error::Unauthorized)?;
		if !bool::from(digest(raw).as_bytes().ct_eq(expected.as_bytes())) {
			return Err(Error::Unauthorized);
		}
		Ok(id
			.strip_suffix("-resource")
			.ok_or(Error::Unauthorized)?
			.to_owned())
	}
	pub(crate) async fn authenticate_client(&self, id: &str, secret: &str) -> Result<()> {
		let client = self
			.oauth
			.client(id)
			.await?
			.filter(|c| c.enabled && c.oidc_enabled)
			.ok_or(Error::Unauthorized)?;
		let hash = client.secret_hash.ok_or(Error::Unauthorized)?;
		let secret = secret.to_owned();
		let verified =
			tokio::task::spawn_blocking(move || Argon2Hasher::new().verify(&secret, &hash))
				.await
				.map_err(|_| Error::Unavailable)??;
		if !verified {
			return Err(Error::Unauthorized);
		}
		Ok(())
	}
	/// Create an independent Desktop credential family from an admitted Cloud
	/// browser. Browser expiry/logout then cannot shorten Desktop's own limits.
	pub(crate) async fn create_desktop_session(
		&self,
		parent_id: Uuid,
		client: &str,
		device_id: Uuid,
		absolute_seconds: u32,
		idle_seconds: u32,
	) -> Result<ProductSession> {
		if client != "cloud"
			|| absolute_seconds == 0
			|| absolute_seconds > 365 * 24 * 3600
			|| idle_seconds == 0
			|| idle_seconds > absolute_seconds
		{
			return Err(Error::Invalid);
		}
		let (validity, _, _) = self.check_session(parent_id).await?;
		if validity != Validity::Active {
			return Err(Error::Unauthorized);
		}
		let now = Utc::now();
		let mut tx = self.store.scope().await?;
		let parent = tx.session(parent_id).await?.ok_or(Error::Unauthorized)?;
		let identity = tx
			.identity(parent.identity_id)
			.await?
			.ok_or(Error::Unauthorized)?;
		let account = tx
			.account(parent.account_id)
			.await?
			.ok_or(Error::Unauthorized)?;
		if tx.session_kind(parent_id).await?.as_deref() != Some("browser")
			|| parent.client_id != client
			|| interactive_validity(&parent, &account, &identity, now) != Validity::Active
		{
			return Err(Error::Unauthorized);
		}
		let session = ProductSession {
			id: Uuid::new_v4(),
			account_id: account.id,
			identity_id: identity.id,
			client_id: client.into(),
			device_id: device_id.to_string(),
			issued_at: parent.issued_at,
			last_seen_at: now,
			absolute_expires_at: now + Duration::seconds(i64::from(absolute_seconds)),
			idle_seconds: i64::from(idle_seconds),
			account_epoch: account.session_epoch,
			revoked_at: None,
		};
		tx.save_session(&session).await?;
		tx.mark_desktop(session.id).await?;
		Box::new(tx).commit().await?;
		Ok(session)
	}

	pub(crate) async fn issue_session(
		&self,
		identity: Uuid,
		client: &str,
		auth_time: chrono::DateTime<Utc>,
	) -> Result<ProductSession> {
		let now = Utc::now();
		let mut tx = self.store.scope().await?;
		let identity = tx.identity(identity).await?.ok_or(Error::Unauthorized)?;
		let account = tx
			.account(identity.first_account.ok_or(Error::Unauthorized)?)
			.await?
			.ok_or(Error::Unauthorized)?;
		if !identity.linked
			|| identity.validity(now) != Validity::Active
			|| account.status != AccountStatus::Active
		{
			return Err(Error::Unauthorized);
		}
		let session = browser_session(&account, &identity, client, auth_time, now);
		tx.save_session(&session).await?;
		Box::new(tx).commit().await?;
		Ok(session)
	}

	/// Continue an admitted SSO browser session into a product session. The
	/// parent is rechecked under the row locks an all-products logout takes, so
	/// a logout that completes after the caller's SSO check leaves no successor.
	pub(crate) async fn continue_sso_session(
		&self,
		parent_id: Uuid,
		client: &str,
	) -> Result<ProductSession> {
		let now = Utc::now();
		let mut tx = self.store.scope().await?;
		let parent = tx.session(parent_id).await?.ok_or(Error::Unauthorized)?;
		let account = tx
			.account(parent.account_id)
			.await?
			.ok_or(Error::Unauthorized)?;
		let identity = tx
			.identity(parent.identity_id)
			.await?
			.ok_or(Error::Unauthorized)?;
		if parent.client_id != "sso"
			|| interactive_validity(&parent, &account, &identity, now) != Validity::Active
		{
			return Err(Error::Unauthorized);
		}
		let session = browser_session(&account, &identity, client, parent.issued_at, now);
		tx.save_session(&session).await?;
		Box::new(tx).commit().await?;
		Ok(session)
	}
}
fn browser_session(
	account: &Account,
	identity: &ExternalIdentity,
	client: &str,
	auth_time: chrono::DateTime<Utc>,
	now: chrono::DateTime<Utc>,
) -> ProductSession {
	ProductSession {
		id: Uuid::new_v4(),
		account_id: account.id,
		identity_id: identity.id,
		client_id: client.into(),
		device_id: secret(),
		issued_at: auth_time,
		last_seen_at: now,
		absolute_expires_at: now + Duration::hours(12),
		idle_seconds: 1800,
		account_epoch: account.session_epoch,
		revoked_at: None,
	}
}
use reinhardt::query::{IntoIden, QueryStatementBuilder};
fn protocol_config(config: &Config) -> Result<(OAuthServerConfig, OidcConfig)> {
	let endpoint = |path| format!("{}{path}", config.issuer);
	let mut oauth = if config.loopback_development {
		OAuthServerConfig::for_loopback_development(
			&config.issuer,
			&endpoint("/oauth/authorize"),
			&endpoint("/oauth/token"),
			&endpoint("/oauth/revoke"),
			&endpoint("/oauth/introspect"),
		)?
	} else {
		OAuthServerConfig::new(
			&config.issuer,
			&endpoint("/oauth/authorize"),
			&endpoint("/oauth/token"),
			&endpoint("/oauth/revoke"),
			&endpoint("/oauth/introspect"),
		)?
	};
	oauth.refresh_tokens = Some(RefreshTokenPolicy::new(
		std::time::Duration::from_secs(1800),
		std::time::Duration::from_secs(43200),
	)?);
	oauth.token_ttl = std::time::Duration::from_secs(300);
	let oidc = if config.loopback_development {
		OidcConfig::for_loopback_development(
			&config.issuer,
			&endpoint("/oauth/authorize"),
			&endpoint("/oauth/token"),
			&endpoint("/oauth/userinfo"),
			&endpoint("/oauth/jwks"),
		)?
	} else {
		OidcConfig::new(
			&config.issuer,
			&endpoint("/oauth/authorize"),
			&endpoint("/oauth/token"),
			&endpoint("/oauth/userinfo"),
			&endpoint("/oauth/jwks"),
		)?
	};
	Ok((oauth, oidc))
}
async fn hash_secret(value: String) -> Result<String> {
	if value.len() < 32 {
		return Err(Error::Configuration);
	}
	tokio::task::spawn_blocking(move || Argon2Hasher::new().hash(&value))
		.await
		.map_err(|_| Error::Unavailable)?
		.map_err(Error::from)
}
struct AccountsAdapter(Store);
struct AuthenticatedAccount(Uuid);
impl AuthIdentity for AuthenticatedAccount {
	fn id(&self) -> String {
		self.0.to_string()
	}
	fn is_authenticated(&self) -> bool {
		true
	}
	fn is_admin(&self) -> bool {
		false
	}
}
#[async_trait]
impl UserRepository for AccountsAdapter {
	async fn get_user_by_id(
		&self,
		id: &str,
	) -> std::result::Result<Option<Box<dyn AuthIdentity>>, String> {
		let Ok(id) = Uuid::parse_str(id) else {
			return Ok(None);
		};
		let mut tx = self.0.scope().await.map_err(|_| "authority unavailable")?;
		Ok(tx
			.account(id)
			.await
			.map_err(|_| "authority unavailable")?
			.filter(|a| a.status == AccountStatus::Active)
			.map(|a| Box::new(AuthenticatedAccount(a.id)) as Box<dyn AuthIdentity>))
	}
}
#[async_trait]
impl OidcAccountStatus for AccountsAdapter {
	async fn is_active(&self, id: &str) -> std::result::Result<bool, String> {
		Ok(self.get_user_by_id(id).await?.is_some())
	}
}
struct SharedLimiter(Store);
#[async_trait]
impl OAuthRateLimiter for SharedLimiter {
	async fn allow(&self, key: &str) -> bool {
		let Ok(mut tx) = self.0.scope().await else {
			return false;
		};
		let Ok(allowed) = tx
			.rate_limit(
				&digest(key),
				if key.contains("/oauth/introspect:")
					|| key.contains("/internal/accounts/check:")
					|| key.contains("/oauth/userinfo:")
				{
					60_000
				} else {
					120
				},
			)
			.await
		else {
			return false;
		};
		Box::new(tx).commit().await.is_ok() && allowed
	}
}
impl SharedOAuthRateLimiter for SharedLimiter {}
