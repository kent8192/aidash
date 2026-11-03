//! Human inspection and control reuse the creator Run's authority.
use crate::{
	Error, Result,
	apps::{
		execution::{
			models::subnodes::SubnodeRecord,
			serializers::subnodes::{Dissolved, MemberRun, SubnodeMember, SubnodeView},
		},
		identity::{
			repositories::{execution::details, subnodes::Management},
			services::{access::Access, identity::SubjectIdentity},
		},
	},
	federation::Federation,
};
use aidash_application::ports::subnodes::ManageScope;
use aidash_domain::{
	registry::EntityRef,
	subnodes::{Allowance, EndReason, State, SubnodeId},
};
use reinhardt::{
	db::orm::Model,
	query::{Alias, Expr, ExprTrait, Order, PostgresQueryBuilder, Query, QueryStatementBuilder},
};
use uuid::Uuid;

pub(crate) async fn list(
	f: &Federation,
	identity: &SubjectIdentity,
	creator: Uuid,
) -> Result<Vec<SubnodeView>> {
	let mut access = Access::begin(&f.store, identity).await?;
	let result = async {
		details::visible_within(f, &mut access, creator).await?;
		let tenant = access.identity.tenant.clone();
		let records = SubnodeRecord::objects()
			.filter(SubnodeRecord::field_tenant().eq(tenant))
			.filter(SubnodeRecord::field_creator_run_id().eq(creator))
			.order_by(&["created_at", "id"])
			.all_with_executor(access.tx.native_mut())
			.await
			.map_err(reinhardt::core::exception::Error::from)?;
		let mut result = Vec::with_capacity(records.len());
		for record in records {
			let subnode = record.domain(access.tx.native_mut()).await?;
			let rows: Vec<(Uuid, String, String, String, String, Uuid)> =
				crate::database::native::query_as(
					&Query::select()
						.columns(
							[
								"id",
								"agent_id",
								"agent_version",
								"phase",
								"control",
								"task_id",
							]
							.map(Alias::new),
						)
						.from(Alias::new("runs"))
						.and_where(Expr::col("workspace_id").eq(Expr::value(subnode.workspace_id)))
						.order_by(Alias::new("id"), Order::Asc)
						.to_string(PostgresQueryBuilder),
				)
				.columns(&[
					"id",
					"agent_id",
					"agent_version",
					"phase",
					"control",
					"task_id",
				])
				.fetch_all(&mut **access.tx)
				.await?;
			let member_runs = rows
				.into_iter()
				.map(
					|(id, agent_id, agent_version, state, control, task_id)| MemberRun {
						id,
						agent: EntityRef {
							id: agent_id,
							version: agent_version,
						},
						state,
						control,
						task_id,
					},
				)
				.collect();
			result.push(SubnodeView {
				id: subnode.id.0,
				title: subnode.title,
				workspace_id: subnode.workspace_id,
				leader: subnode.leader,
				members: subnode
					.members
					.into_iter()
					.map(|member| SubnodeMember {
						agent_id: member.agent_id,
						agent_version: member.agent_version,
						added_by_run_id: member.added_by_run_id,
						added_at: member.added_at,
					})
					.collect(),
				allowance: subnode.allowance,
				state: match subnode.state {
					State::Active => "active",
					State::Dissolved => "dissolved",
				}
				.into(),
				created_at: subnode.created_at,
				ended_at: subnode.ended_at,
				ended_reason: subnode.ended_reason.and_then(|reason| {
					serde_json::to_value(reason)
						.ok()
						.and_then(|value| value.as_str().map(str::to_owned))
				}),
				member_runs,
			});
		}
		Ok(result)
	}
	.await;
	access.finish(result).await
}

pub(crate) async fn dissolve(
	f: &Federation,
	identity: &SubjectIdentity,
	id: Uuid,
) -> Result<Dissolved> {
	let mut access = Access::begin(&f.store, identity).await?;
	let result = async {
		let tenant = access.identity.tenant.clone();
		let record = SubnodeRecord::objects()
			.filter(SubnodeRecord::field_tenant().eq(tenant))
			.filter(SubnodeRecord::field_id().eq(id))
			.all_with_executor(access.tx.native_mut())
			.await
			.map_err(reinhardt::core::exception::Error::from)?
			.pop()
			.ok_or(Error::Forbidden)?;
		let creator = super::management::authorized(
			&mut access,
			record.creator_run_id,
			f.config.deployment.as_str(),
		)
		.await?;
		let mut scope = Management {
			access: &mut access,
			federation: f,
			creator_authority: Allowance::default(),
			candidate_authority: None,
			message_limit: 0,
		};
		let locked = scope.lock(SubnodeId(id)).await?.ok_or(Error::Forbidden)?;
		if locked.creator_run_id != creator.id {
			return Err(Error::Forbidden);
		}
		if locked.state == State::Active {
			scope.dissolve(&locked, EndReason::Dissolved).await?;
		}
		Ok(Dissolved { dissolved: true })
	}
	.await;
	let result = access.finish(result).await?;
	f.notify.notify_waiters();
	Ok(result)
}
