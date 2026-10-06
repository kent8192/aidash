//! PostgreSQL authority scope shared by entry, retrieval, and indexing ports.
use crate::apps::knowledge::{
	serializers::service::SavedAuthority,
	services::core::{Entry, Source},
};
use crate::authorization::{
	access::Access,
	identity::{Actor, SubjectIdentity},
};
use crate::{Error, Result, store::Store};
use serde_json::Value;

use uuid::Uuid;

pub(crate) enum Lease<'a> {
	Operator(crate::database::native::Transaction),
	Scoped(Box<Access>),
	Inherited(&'a mut Access),
}
impl Lease<'_> {
	pub(crate) async fn begin(store: &Store, actor: &Actor) -> Result<Self> {
		Ok(match actor {
			Actor::Operator => Self::Operator(crate::database::native::begin(&store.pool).await?),
			Actor::Subject(identity) => {
				Self::Scoped(Box::new(Access::begin(store, identity).await?))
			}
		})
	}
	pub(crate) async fn restore(store: &Store, authority: Value) -> Result<Self> {
		let authority: SavedAuthority = serde_json::from_value(authority)?;
		match authority.credential {
			None if authority.subject == "operator"
				&& authority.tenant.is_empty()
				&& authority.subjects.is_empty() =>
			{
				Self::begin(store, &Actor::Operator).await
			}
			Some(credential_id) => {
				let mut access = Access::begin(
					store,
					&SubjectIdentity {
						http_session: None,
						credential_id,
						tenant: authority.tenant,
						subject: authority.subject,
					},
				)
				.await?;
				if authority.subjects.is_empty()
					|| !authority.subjects.contains(&access.identity.subject)
				{
					return Err(Error::Forbidden);
				}
				access.subjects = authority.subjects;
				access.worker();
				Ok(Self::Scoped(Box::new(access)))
			}
			_ => Err(Error::Forbidden),
		}
	}
	pub(crate) fn tx(&mut self) -> &mut crate::database::native::Transaction {
		match self {
			Self::Operator(tx) => tx,
			Self::Scoped(a) => &mut a.tx,
			Self::Inherited(a) => &mut a.tx,
		}
	}
	pub(crate) fn access(&mut self) -> Option<&mut Access> {
		match self {
			Self::Operator(_) => None,
			Self::Scoped(a) => Some(a),
			Self::Inherited(a) => Some(a),
		}
	}
	pub(crate) fn durable(&mut self) {
		if let Some(access) = self.access() {
			access.durable_audit = true;
		}
	}
	pub(crate) fn saved(&mut self) -> Result<Value> {
		let saved = match self.access() {
			None => SavedAuthority {
				credential: None,
				tenant: String::new(),
				subject: "operator".into(),
				subjects: vec![],
			},
			Some(a) => SavedAuthority {
				credential: Some(a.identity.credential_id),
				tenant: a.identity.tenant.clone(),
				subject: a.identity.subject.clone(),
				subjects: a.subjects.clone(),
			},
		};
		Ok(serde_json::to_value(saved)?)
	}
	pub(crate) async fn finish<T>(self, result: Result<T>) -> Result<T> {
		match self {
			Self::Operator(tx) => {
				if result.is_ok() {
					tx.commit().await?;
				} else {
					tx.rollback().await?;
				}
				result
			}
			Self::Scoped(a) => a.finish(result).await,
			Self::Inherited(_) => result,
		}
	}
	pub(crate) async fn workspace(&mut self, workspace: Uuid, action: &str) -> Result<()> {
		if self.access().is_none() && action != "semantic.index.manage" {
			crate::authorization::remote::operator::require(self.tx(), workspace).await?;
		}
		if let Some(a) = self.access() {
			let resource = a.workspace(workspace).await?;
			a.require(&resource, "workspace.read").await?;
			a.require(&resource, action).await?;
		}
		Ok(())
	}
	pub(crate) async fn permits(&mut self, entry: &Entry, action: &str) -> Result<bool> {
		aidash_application::semantic::visibility::permits(
			&mut crate::bootstrap::semantic_disclosure_scope(self),
			&entry.clone().into(),
			action,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn source(
		&mut self,
		workspace: Uuid,
		source: &Source,
	) -> Result<Option<String>> {
		aidash_application::semantic::visibility::source(
			&mut crate::bootstrap::semantic_disclosure_scope(self),
			workspace,
			source,
		)
		.await
		.map_err(Into::into)
	}
}
