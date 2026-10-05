//! Keep the current credential/policy lease through the conversation transaction.
use crate::{
	Error, Result,
	authorization::{access::Access, identity::Actor},
	domain::Message,
	store::Store,
};
use reinhardt::query::{Alias, Expr, LockType, PostgresQueryBuilder, Query};
use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};
use serde_json::json;

use uuid::Uuid;

pub(crate) enum Lease {
	Scoped(Box<Access>),
	Operator(crate::database::native::Transaction),
}

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
		match actor {
			Actor::Subject(identity) => {
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
				Ok(Self::Scoped(Box::new(access)))
			}
			Actor::Operator => {
				let mut tx = crate::database::native::begin(&store.pool).await?;
				crate::authorization::remote::operator::require(&mut tx, workspace).await?;
				let exists: Option<Uuid> = {
					let query_bind_1 = workspace;
					crate::database::native::query_scalar(
						&Query::select()
							.column(Alias::new("id"))
							.from(Alias::new("workspaces"))
							.and_where(
								reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
									SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_1.to_owned()).into()],
									),
								),
							)
							.lock(LockType::Share)
							.to_string(PostgresQueryBuilder),
					)
					.scalar_optional(&mut *tx)
					.await?
				};
				if exists.is_none() {
					return Err(Error::NotFound("channel unavailable".into()));
				}
				Ok(Self::Operator(tx))
			}
		}
	}

	pub fn tx(&mut self) -> &mut crate::database::native::Transaction {
		match self {
			Self::Scoped(access) => &mut access.tx,
			Self::Operator(tx) => tx,
		}
	}

	pub fn access_mut(&mut self) -> Option<&mut Access> {
		match self {
			Self::Scoped(access) => Some(access.as_mut()),
			Self::Operator(_) => None,
		}
	}

	pub fn sender(&self) -> String {
		match self {
			Self::Scoped(access) => access.identity.subject.clone(),
			Self::Operator(_) => "human".into(),
		}
	}

	pub fn principal(&self) -> String {
		match self {
			Self::Scoped(access) => crate::registry::digest(&json!({
				"tenant":access.identity.tenant, "subject":access.identity.subject,
			})),
			Self::Operator(_) => "operator".into(),
		}
	}

	pub async fn visible(&mut self, message: &Message) -> Result<bool> {
		match self {
			Self::Scoped(access) => access.message_visible(message).await,
			Self::Operator(_) => Ok(true),
		}
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
							.eq(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							)),
					)
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
							SimpleExpr::CustomWithExpr(
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
		match self {
			Self::Scoped(access) => (*access).finish(result).await,
			Self::Operator(tx) => match result {
				Ok(value) => {
					tx.commit().await?;
					Ok(value)
				}
				Err(error) => {
					tx.rollback().await?;
					Err(error)
				}
			},
		}
	}
}

use reinhardt::query::ColumnRef;

use crate::authorization::access::NativeAccess;
use reinhardt::db::backends::TransactionExecutor;
pub(crate) enum NativeLease {
	Scoped(Box<NativeAccess>),
	Operator(Box<dyn TransactionExecutor>),
}
impl Lease {
	pub(crate) fn into_native(self) -> Result<NativeLease> {
		match self {
			Self::Scoped(access) => Ok(NativeLease::Scoped(Box::new((*access).into_native()?))),
			Self::Operator(tx) => Ok(NativeLease::Operator(tx.into_executor())),
		}
	}
}
impl NativeLease {
	pub(crate) fn tx(&mut self) -> &mut dyn TransactionExecutor {
		match self {
			Self::Scoped(access) => access.tx.as_mut(),
			Self::Operator(tx) => tx.as_mut(),
		}
	}
	pub(crate) async fn finish<T>(self, result: Result<T>) -> Result<T> {
		match self {
			Self::Scoped(access) => (*access).finish(result).await,
			Self::Operator(tx) => match result {
				Ok(value) => {
					tx.commit().await?;
					Ok(value)
				}
				Err(error) => {
					tx.rollback().await?;
					Err(error)
				}
			},
		}
	}
}

use reinhardt::query::SimpleExpr;
