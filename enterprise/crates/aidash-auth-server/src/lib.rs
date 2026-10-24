//! Standalone shared human authentication. Product authority remains product-local.
pub mod config;
pub mod grpc;
pub mod http;
pub mod migrations;
mod models;
pub mod service;
pub mod store;

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error(transparent)]
	Application(#[from] aidash_application::Error),
	#[error(transparent)]
	Accounts(#[from] aidash_application::accounts::Error),
	#[error(transparent)]
	Framework(#[from] reinhardt::core::exception::Error),
	#[error("OIDC operation failed: {0:?}")]
	Oidc(reinhardt::auth::oidc_op::OidcError),
	#[error(transparent)]
	OAuth(#[from] reinhardt::auth::oauth2_server::OAuthError),
	#[error("authentication configuration unavailable")]
	Configuration,
	#[error("authentication is not valid")]
	Unauthorized,
	#[error("authentication operation is forbidden")]
	Forbidden,
	#[error("invalid authentication request")]
	Invalid,
	#[error("authentication authority unavailable")]
	Unavailable,
}

#[cfg(test)]
mod tests;

impl From<reinhardt::auth::oidc_op::OidcError> for Error {
	fn from(value: reinhardt::auth::oidc_op::OidcError) -> Self {
		Self::Oidc(value)
	}
}
