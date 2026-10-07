use crate::apps::identity::{
	Authorization, Snapshot,
	catalog::Binding,
	identity::{Credential, IssuedCredential},
	policy::{Decision, Evaluation},
};
use crate::{Result, federation::Federation};
use reinhardt::injectable;

use serde_json::Value;

use uuid::Uuid;

fn service(f: Federation) -> Authorization {
	Authorization { pool: f.store.pool }
}

fn control_service(f: Federation) -> Authorization {
	Authorization {
		pool: f.store.control_pool,
	}
}

use crate::apps::identity::serializers::policies::{
	AuthorizationPage, AuthorizationUpdate, CatalogInput, CredentialInput, CredentialRevocation,
	PolicyReplacement,
};

/// Mounted inside the management router, behind the existing operator token.
/// Evaluation bodies are simulations of authority, never execution credentials.
#[derive(Clone)]
pub struct PolicyAdministration {
	runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> PolicyAdministration {
	PolicyAdministration { runtime }
}

impl PolicyAdministration {
	pub async fn catalog(&self, tenant: String) -> Result<Vec<Binding>> {
		let f = self.runtime.clone();
		service(f).catalog(&tenant).await
	}
	pub(crate) async fn set_catalog(&self, tenant: String, input: CatalogInput) -> Result<Binding> {
		let f = self.runtime.clone();
		service(f)
			.set_catalog(
				&tenant,
				&input.entry,
				input.expected_revision,
				input.enabled,
				"operator",
			)
			.await
	}
	pub(crate) async fn issue_credential(
		&self,
		tenant: String,
		input: CredentialInput,
	) -> Result<IssuedCredential> {
		let f = self.runtime.clone();
		service(f)
			.issue_credential(
				&tenant,
				&input.subject,
				input.expires_in_seconds,
				"operator",
			)
			.await
	}
	pub async fn credentials(
		&self,
		tenant: String,
		page: crate::apps::workspaces::serializers::tasks::PageQuery,
	) -> Result<Vec<Credential>> {
		let f = self.runtime.clone();
		control_service(f)
			.credentials_page(&tenant, page.offset)
			.await
	}
	pub(crate) async fn revoke_credential(
		&self,
		(tenant, id): (String, Uuid),
	) -> Result<(reinhardt::StatusCode, CredentialRevocation)> {
		let f = self.runtime.clone();
		let credential = control_service(f.clone())
			.revoke_credential(&tenant, id)
			.await?;
		let pending_transactions =
			crate::transactions::authority::pending(&f, &tenant, Some(id)).await?;
		let status = if pending_transactions.is_empty() {
			reinhardt::StatusCode::OK
		} else {
			reinhardt::StatusCode::ACCEPTED
		};
		Ok((
			status,
			CredentialRevocation {
				credential,
				pending_transactions,
			},
		))
	}
	pub async fn snapshot(&self, tenant: String) -> Result<Snapshot> {
		let f = self.runtime.clone();
		control_service(f).snapshot(&tenant).await
	}
	pub(crate) async fn replace(
		&self,
		tenant: String,
		input: AuthorizationUpdate,
	) -> Result<(reinhardt::StatusCode, PolicyReplacement)> {
		let f = self.runtime.clone();
		let (snapshot, pending_host_packages) = control_service(f.clone())
			.replace_with_host_defaults(
				&f.store,
				&f.config.default_host_packages,
				&tenant,
				input.expected_revision,
				input.bundle,
				"operator",
			)
			.await?;
		let pending_transactions =
			crate::transactions::authority::pending(&f, &tenant, None).await?;
		let status = if pending_transactions.is_empty() {
			reinhardt::StatusCode::OK
		} else {
			reinhardt::StatusCode::ACCEPTED
		};
		Ok((
			status,
			PolicyReplacement {
				snapshot,
				pending_host_packages,
				pending_transactions,
			},
		))
	}
	pub async fn transaction_revocations(&self, tenant: String) -> Result<Vec<Uuid>> {
		crate::transactions::authority::pending(&self.runtime, &tenant, None).await
	}
	pub async fn evaluate(&self, tenant: String, input: Evaluation) -> Result<Decision> {
		let f = self.runtime.clone();
		control_service(f).evaluate(&tenant, &input).await
	}
	pub async fn simulate(&self, tenant: String, input: Evaluation) -> Result<Decision> {
		let f = self.runtime.clone();
		control_service(f).simulate(&tenant, &input).await
	}
	pub(crate) async fn revisions(
		&self,
		tenant: String,
		page: AuthorizationPage,
	) -> Result<Vec<Value>> {
		let f = self.runtime.clone();
		service(f).revisions(&tenant, page.after, page.limit).await
	}
	pub(crate) async fn decisions(
		&self,
		tenant: String,
		page: AuthorizationPage,
	) -> Result<Vec<Value>> {
		let f = self.runtime.clone();
		service(f).decisions(&tenant, page.after, page.limit).await
	}
}
