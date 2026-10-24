//! One full-stack Reinhardt Pages Authentication project.
pub mod apps;
#[cfg(client)]
pub mod client;
pub mod config;
#[cfg(server)]
pub(crate) use apps::authentication::models;
#[cfg(client)]
pub(crate) use apps::authentication::{
	client::components::{app, view},
	serializers as model,
	services::client::{api, bridge},
};
#[cfg(server)]
pub use apps::authentication::{
	server::{grpc, migrations, store, views as http},
	services::server as service,
};
#[cfg(server)]
mod native {
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

	impl From<reinhardt::auth::oidc_op::OidcError> for Error {
		fn from(value: reinhardt::auth::oidc_op::OidcError) -> Self {
			Self::Oidc(value)
		}
	}
}
#[cfg(server)]
pub use native::{Error, Result};
