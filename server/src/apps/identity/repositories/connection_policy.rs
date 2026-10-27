//! Live registered clients and scope ceilings used when beginning a Connection.
use super::oauth_clients::NativeRepository;
use aidash_application::{
	Result,
	oauth_clients::{Repository, normalize_issuer},
	ports::oauth::ConnectionPolicy,
};
use aidash_domain::oauth_connections::{OAuthClient, canonical_resource};
use async_trait::async_trait;
#[derive(Clone)]
pub struct NativeConnectionPolicy {
	pub pool: crate::database::native::Pool,
}
#[async_trait]
impl ConnectionPolicy for NativeConnectionPolicy {
	async fn registered_clients(
		&self,
		tenant: &str,
		subject: &str,
		issuer: &str,
	) -> Result<(Option<OAuthClient>, Option<OAuthClient>)> {
		let issuer = normalize_issuer(issuer)?;
		let mut tx = NativeRepository {
			pool: self.pool.clone(),
		}
		.begin(tenant)
		.await?;
		let rows = tx.clients().await?;
		tx.commit().await?;
		let mut personal = None;
		let mut shared = None;
		for row in rows.into_iter().filter(|row| {
			row.issuer == issuer
				&& row.revision > 0
				&& (row.secret_resource.is_none() || row.pinned_version.is_some())
		}) {
			if row.owner.as_deref() == Some(subject) {
				personal = Some(row);
			} else if row.owner.is_none() {
				shared = Some(row);
			}
		}
		Ok((personal, shared))
	}
	async fn scope_ceiling(&self, tenant: &str, resource: &str) -> Result<Option<Vec<String>>> {
		let resource = canonical_resource(resource)
			.map_err(|error| aidash_application::Error::Invalid(error.to_string()))?;
		let mut tx = NativeRepository {
			pool: self.pool.clone(),
		}
		.begin(tenant)
		.await?;
		let row = tx.ceiling(&resource).await?;
		tx.commit().await?;
		Ok(row.map(|row| row.scopes))
	}
}
