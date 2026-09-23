//! Keep the current credential/policy lease through the conversation transaction.
use crate::{
	Error, Result,
	authorization::{access::Access, identity::Actor},
	domain::Message,
	store::Store,
};
use sea_orm::sea_query::{Alias, Asterisk, Expr, LockType, PostgresQueryBuilder, Query};
use serde_json::json;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub(crate) enum Lease {
	Scoped(Box<Access>),
	Operator(Transaction<'static, Postgres>),
}

impl Lease {
	pub async fn begin(store: &Store, actor: Actor, workspace: Uuid, action: &str) -> Result<Self> {
		match actor {
			Actor::Subject(identity) => {
				let mut access = Access::begin(store, &identity).await?;
				let result = async {
					let resource = access.workspace(workspace).await?;
					access.require(&resource, "workspace.read").await?;
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
				let mut tx = store.pool.begin().await?;
				let exists: Option<Uuid> = sqlx::query_scalar(
					&Query::select()
						.column(Alias::new("id"))
						.from(Alias::new("workspaces"))
						.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
						.lock(LockType::Share)
						.to_string(PostgresQueryBuilder),
				)
				.bind(workspace)
				.fetch_optional(&mut *tx)
				.await?;
				if exists.is_none() {
					return Err(Error::NotFound("channel unavailable".into()));
				}
				Ok(Self::Operator(tx))
			}
		}
	}

	pub fn tx(&mut self) -> &mut Transaction<'static, Postgres> {
		match self {
			Self::Scoped(access) => &mut access.tx,
			Self::Operator(tx) => tx,
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
		let message: Option<Message> = sqlx::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("messages"))
				.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(workspace)
		.bind(id)
		.fetch_optional(&mut **self.tx())
		.await?;
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
