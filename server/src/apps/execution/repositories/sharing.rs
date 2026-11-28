//! Tenant serialization for legacy records and Thread deletion.
use crate::apps::execution::capabilities::serializers::sharing::Recipient as NativeRecipient;
use crate::{Result as NativeResult, authorization::access::Access};
use aidash_domain::capabilities::sharing::Recipient;
use reinhardt::query::{Expr, PostgresQueryBuilder, Query, QueryStatementBuilder as _, SimpleExpr};
pub(crate) async fn serialize(access: &mut Access) -> NativeResult<()> {
	{
		let query_bind_1 = format!("core-share:{}", access.identity.tenant);
		crate::database::native::query(
			&Query::select()
				.expr(SimpleExpr::CustomWithExpr(
					"(pg_advisory_xact_lock(hashtextextended(?, 0)))".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **access.tx)
		.await?
	};
	Ok(())
}
impl From<NativeRecipient> for Recipient {
	fn from(v: NativeRecipient) -> Self {
		Self {
			node_id: v.node_id,
			agent_id: v.agent_id,
			agent_version: v.agent_version,
			thread_id: v.thread_id,
		}
	}
}
impl From<Recipient> for NativeRecipient {
	fn from(v: Recipient) -> Self {
		Self {
			node_id: v.node_id,
			agent_id: v.agent_id,
			agent_version: v.agent_version,
			thread_id: v.thread_id,
		}
	}
}
