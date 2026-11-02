//! Transactional, Tenant-authorized Scratch Space operations.
use super::super::{
	repositories::scratch::{NativeScratchServices, ScratchAdapter},
	serializers::scratch::{ScratchCreate, ScratchSettingsUpdate},
};
use crate::{
	Error, Result,
	authorization::{access::Access, identity::Actor},
	federation::Federation,
};
use aidash_application::scratch as app;
use aidash_application::scratch::ScratchScope;
use aidash_domain::{
	nodes::NodeName,
	scratch::{ScratchSettings, ScratchSpace, ScratchState},
};
use chrono::Utc;
use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) enum Operation {
	Create(ScratchCreate),
	List(NodeName),
	Limits(NodeName),
	Read(Uuid),
	Settings(Uuid, ScratchSettingsUpdate),
	AddMember(Uuid, String),
	RemoveMember(Uuid, String),
	Clear(Uuid),
	Delete(Uuid),
}

pub(crate) async fn execute(f: &Federation, actor: Actor, op: Operation) -> Result<Value> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	let mut access = Access::begin(&f.store, &identity).await?;
	let mutation = !matches!(
		&op,
		Operation::List(_) | Operation::Read(_) | Operation::Limits(_)
	);
	let result: Result<Value> = async {
		let mut services = NativeScratchServices {
			policy: &f.config.scratch_policy,
		};
		let mut scope = ScratchAdapter {
			access: &mut access,
			services: &mut services,
			person_override: None,
		};
		let now = Utc::now();
		let value = match op {
			Operation::Create(input) => {
				let settings = if let Some(settings) = input.settings {
					settings
				} else {
					let (_, tenant_hosts) = scope.limits(&input.node).await?;
					ScratchSettings::for_tenant(&tenant_hosts)
				};
				let space = ScratchSpace {
					id: Uuid::new_v4(),
					node: input.node,
					name: input.name,
					creator: identity.subject.clone(),
					members: Default::default(),
					state: ScratchState::Active,
					revision: 0,
					wipe_generation: 0,
					settings,
					created_at: now,
					last_used_at: now,
				};
				app::create(&mut scope, &space).await?;
				serde_json::to_value(space)?
			}
			Operation::List(node) => {
				serde_json::to_value(app::list(&mut scope, &node, now).await?)?
			}
			Operation::Limits(node) => {
				let person = scope.person().to_owned();
				scope.require_node_use(&node, &person).await?;
				let (ceilings, tenant_hosts) = scope.limits(&node).await?;
				json!({"ceilings": ceilings, "tenant_hosts": tenant_hosts})
			}
			Operation::Read(id) => serde_json::to_value(app::read(&mut scope, id, now).await?)?,
			Operation::Settings(id, value) => {
				app::update_settings(&mut scope, id, now, value.revision, value.settings).await?;
				json!({"sent":true})
			}
			Operation::AddMember(id, subject) => {
				app::add_member(&mut scope, id, now, &subject).await?;
				json!({"sent":true})
			}
			Operation::RemoveMember(id, subject) => {
				app::remove_member(&mut scope, id, now, &subject).await?;
				json!({"sent":true})
			}
			Operation::Clear(id) => {
				app::clear(&mut scope, id, now).await?;
				json!({"sent":true})
			}
			Operation::Delete(id) => {
				app::delete(&mut scope, id, now).await?;
				json!({"sent":true})
			}
		};
		Ok(value)
	}
	.await;
	let response = access.finish(result).await?;
	if mutation {
		f.notify.notify_waiters();
	}
	Ok(response)
}
