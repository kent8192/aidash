//! Duty templates in the Marketplace (#1011 D4): publishing one of a
//! person's Duties as a package, setting up a Duty from one, and moving a
//! Duty to a later version of its template. Each request is one transaction
//! that takes the Marketplace distribution lock first, as every Marketplace
//! request does, and the Tenant's Duty registration lock after it. The
//! Marketplace and Duty adapters borrow the same Access one after the other.
use super::{management::handoff, storage};
use crate::apps::marketplace::serializers::duty_templates::{
	InstallDutyTemplate, InstalledDuty, PublishDutyTemplate, UpdateDutyTemplate,
};
use crate::apps::workspaces::{
	repositories::{duties as duty_repository, duty_events},
	serializers::duties::DutyView,
	services::duties::Adapter,
};
use crate::{
	Error, Result,
	authorization::{
		access::Access,
		identity::{Actor, SubjectIdentity},
	},
	federation::Federation,
};
use aidash_application::{
	duties::{self as duty_app, DutyScope as _},
	marketplace::duty_templates::{self as templates, PublishTemplate},
};
use aidash_domain::duties::{
	Duty,
	events::TriggerKind,
	templates::{Source, Template},
};
use chrono::Utc;
use reinhardt::Response;
use serde_json::Value;
use uuid::Uuid;

fn subject(actor: &Actor) -> Result<&SubjectIdentity> {
	match actor {
		Actor::Subject(identity) => Ok(identity),
		Actor::DeploymentAdministrator => Err(Error::Forbidden),
	}
}

/// The person's own Duty and the kinds of its triggers.
async fn own_duty(
	store: &crate::store::Store,
	access: &mut Access,
	id: Uuid,
) -> Result<(Duty, Vec<TriggerKind>)> {
	let duty = duty_app::own(
		&mut Adapter {
			access: &mut *access,
			store,
			now: Utc::now(),
		},
		id,
	)
	.await?;
	let tenant = access.identity.tenant.clone();
	let kinds = duty_events::triggers(&mut *access.tx, &tenant, duty.id)
		.await?
		.into_iter()
		.map(|trigger| trigger.kind)
		.collect();
	Ok((duty, kinds))
}

/// The view of a Duty for the person acting.
async fn view(store: &crate::store::Store, access: &mut Access, duty: Duty) -> Result<DutyView> {
	let mut scope = Adapter {
		access: &mut *access,
		store,
		now: Utc::now(),
	};
	let administers = scope.administers().await?;
	let viewer = scope.subject().to_owned();
	Ok(DutyView::new(duty, &viewer, administers))
}

/// The template one of the person's own Duties would publish as: its work,
/// its time trigger and the kinds of its other triggers, with empty
/// instructions for the person to write.
pub(crate) async fn draft(f: &Federation, actor: Actor, id: Uuid) -> Result<Value> {
	let mut access = Access::begin(&f.store, subject(&actor)?).await?;
	let result = async {
		let (duty, kinds) = own_duty(&f.store, &mut access, id).await?;
		Ok(serde_json::to_value(Template::draft(
			&duty.definition,
			&kinds,
			true,
		))?)
	}
	.await;
	access.finish(result).await
}

/// Publishes one of the person's own Duties as a Marketplace template. It
/// needs `marketplace.publish`, like every package, and is shared with the
/// person's Tenant alone until its owner shares it further.
pub(crate) async fn publish(
	f: &Federation,
	actor: Actor,
	id: Uuid,
	input: PublishDutyTemplate,
) -> Result<Response> {
	let mut access = storage::begin(&f.store, subject(&actor)?, true).await?;
	let result = async {
		let (duty, kinds) = own_duty(&f.store, &mut access, id).await?;
		let template = Template::from_duty(
			&duty.definition,
			&kinds,
			&input.triggers,
			input.suggest_agent,
		)?;
		Ok(templates::publish(
			&mut crate::bootstrap::marketplace_publication_scope(&f.store, &mut access),
			&template,
			&PublishTemplate {
				package_id: input.package_id,
				version: input.version,
				author: input.author,
				description: input.description,
				idempotency_key: input.idempotency_key,
			},
			f.store.deployment.as_str(),
			Utc::now(),
		)
		.await?)
	}
	.await;
	handoff(&f.store, access, result).await
}

/// Sets up a Duty from a Marketplace template in a Workspace: the person is
/// its Requester, and the response lists the triggers they create on it. A
/// retried request returns the Duty the first one set up.
pub(crate) async fn install(
	f: &Federation,
	actor: Actor,
	key: String,
	input: InstallDutyTemplate,
) -> Result<Response> {
	let mut access = storage::begin(&f.store, subject(&actor)?, false).await?;
	let result = async {
		let now = Utc::now();
		let (version, template) = templates::installable(
			&mut crate::bootstrap::marketplace_distribution_scope(&mut access),
			&key,
			&input.digest,
			f.store.deployment.as_str(),
			now,
		)
		.await?;
		let (tenant, requester) = (
			access.identity.tenant.clone(),
			access.identity.subject.clone(),
		);
		duty_repository::lock_registrations(&mut *access.tx, &tenant).await?;
		let duty = match duty_repository::installed(
			&mut *access.tx,
			&tenant,
			&requester,
			input.idempotency_key,
		)
		.await?
		{
			Some(duty)
				if duty.workspace_id == input.workspace_id
					&& duty
						.template
						.as_ref()
						.is_some_and(|source| source.key == version.key) =>
			{
				duty
			}
			Some(_) => {
				return Err(Error::Conflict(
					"idempotency key has different input".into(),
				));
			}
			None => {
				duty_app::install_template(
					&mut Adapter {
						access: &mut access,
						store: &f.store,
						now,
					},
					input.workspace_id,
					&template,
					input.agent,
					Source::of(&version, Some(input.idempotency_key)),
				)
				.await?
			}
		};
		Ok(InstalledDuty {
			duty: view(&f.store, &mut access, duty).await?,
			triggers_to_create: template.to_create(),
		})
	}
	.await;
	handoff(&f.store, access, result).await
}

/// Moves a Duty installed from a template to a later version of it that the
/// person may read and install, as the Duty's Requester's own edit.
pub(crate) async fn update(
	f: &Federation,
	actor: Actor,
	id: Uuid,
	input: UpdateDutyTemplate,
) -> Result<Response> {
	let mut access = storage::begin(&f.store, subject(&actor)?, false).await?;
	let result = async {
		let now = Utc::now();
		let (version, template) = templates::installable(
			&mut crate::bootstrap::marketplace_distribution_scope(&mut access),
			&input.key,
			&input.digest,
			f.store.deployment.as_str(),
			now,
		)
		.await?;
		let duty = duty_app::update_from_template(
			&mut Adapter {
				access: &mut access,
				store: &f.store,
				now,
			},
			id,
			&template,
			Source::of(&version, None),
		)
		.await?;
		view(&f.store, &mut access, duty).await
	}
	.await;
	handoff(&f.store, access, result).await
}

/// The latest later version of the template a Duty came from that the
/// person may read and install, for the Duty's page to offer.
pub(crate) async fn newer(
	access: &mut Access,
	node: &str,
	source: &Source,
) -> Result<Option<Source>> {
	storage::lock(&mut access.tx, false).await?;
	Ok(templates::newer(
		&mut crate::bootstrap::marketplace_distribution_scope(access),
		source,
		node,
	)
	.await?)
}
