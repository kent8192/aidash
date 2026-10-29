//! Keep the current credential/policy lease through the conversation transaction.
use crate::{
	Error, Result,
	authorization::{access::Access, identity::Actor},
	domain::Message,
	store::Store,
};
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};
use serde_json::json;

use uuid::Uuid;

/// The lease of a Subject. Workspaces are Tenant content: a Deployment
/// Administrator holds none (#484).
pub(crate) struct Lease(Box<Access>);

impl Lease {
	pub async fn begin(store: &Store, actor: Actor, workspace: Uuid, action: &str) -> Result<Self> {
		Self::begin_with_workspace_read(store, actor, workspace, action, true).await
	}

	pub async fn begin_message_create(
		store: &Store,
		actor: Actor,
		workspace: Uuid,
	) -> Result<Self> {
		Self::begin_with_workspace_read(store, actor, workspace, "message.create", false).await
	}

	async fn begin_with_workspace_read(
		store: &Store,
		actor: Actor,
		workspace: Uuid,
		action: &str,
		require_workspace_read: bool,
	) -> Result<Self> {
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		let mut access = Access::begin(store, &identity).await?;
		let result = async {
			let resource = access.workspace(workspace).await?;
			if require_workspace_read || action == "workspace.read" {
				access.require(&resource, "workspace.read").await?;
			}
			if action != "workspace.read" {
				access.require(&resource, action).await?;
			}
			Ok(())
		}
		.await;
		if let Err(error) = result {
			let error = if action == "workspace.read" && matches!(error, Error::Forbidden) {
				Error::NotFound("channel unavailable".into())
			} else {
				error
			};
			return access.finish(Err(error)).await;
		}
		Ok(Self(Box::new(access)))
	}

	pub fn tx(&mut self) -> &mut crate::database::native::Transaction {
		&mut self.0.tx
	}

	pub fn access_mut(&mut self) -> &mut Access {
		&mut self.0
	}

	pub fn sender(&self) -> String {
		self.0.identity.subject.clone()
	}

	pub fn principal(&self) -> String {
		crate::registry::digest(&json!({
			"tenant":self.0.identity.tenant, "subject":self.0.identity.subject,
		}))
	}

	pub async fn visible(&mut self, message: &Message) -> Result<bool> {
		self.0.message_visible(message).await
	}

	pub async fn message(&mut self, workspace: Uuid, id: Uuid) -> Result<Message> {
		let message: Option<Message> = {
			let query_bind_1 = workspace;
			let query_bind_2 = id;
			aidash_server::database::query_as(
				&Query::select()
					.column(ColumnRef::Asterisk)
					.from(Alias::new("messages"))
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("workspace_id")))
							.eq(reinhardt::query::SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							)),
					)
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
							reinhardt::query::SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							),
						),
					)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **self.tx())
			.await?
		};
		match message {
			Some(message) if self.visible(&message).await? => Ok(message),
			_ => Err(Error::NotFound("message unavailable".into())),
		}
	}

	pub async fn finish<T>(self, result: Result<T>) -> Result<T> {
		(*self.0).finish(result).await
	}
}

use reinhardt::query::ColumnRef;

use crate::authorization::access::NativeAccess;
use reinhardt::db::backends::TransactionExecutor;
pub(crate) struct NativeLease(Box<NativeAccess>);
impl Lease {
	pub(crate) async fn into_native(self) -> Result<NativeLease> {
		Ok(NativeLease(Box::new((*self.0).into_native()?)))
	}
}
impl NativeLease {
	pub(crate) fn tx(&mut self) -> &mut dyn TransactionExecutor {
		self.0.tx.as_mut()
	}
	pub(crate) async fn finish<T>(self, result: Result<T>) -> Result<T> {
		(*self.0).finish(result).await
	}
}
