//! Budgets (#1012, ADR 0143). Only a person's browser session sets them:
//! `administrator` refuses Subject Credentials, so no Agent changes a Budget
//! even when its root person could.
use super::tenant_administration::TenantAdministrations;
use crate::apps::identity::{
	models::TenantAdministrator,
	oidc::BrowserOrigin,
	repositories::budgets::{self as repository, Budget},
	serializers::budgets::{BudgetUpdate, BudgetView},
};
use crate::{
	Error, Result,
	authorization::{Snapshot, identity::Actor},
};
use aidash_domain::budgets::{Definition, Scope};
use aidash_domain::identity::tenant_administration::{BUDGET_MANAGE, NODE_ADMINISTER};
use chrono::Utc;
use reinhardt::db::backends::TransactionExecutor;
use serde_json::json;
use uuid::Uuid;

/// Who may set a Budget: anyone with `budget.manage`; a Node's administrator
/// for that Node; a Duty's Requester for that Duty; a person for themself.
/// Only `budget.manage` changes or removes a Budget someone else set last,
/// so a person cannot lift a limit an administrator put on them (`existing`
/// is the stored Budget being changed). A Duty or Node that no longer exists
/// gives nobody a scope right; `budget.manage` still reaches its Budget.
async fn may_manage(
	tx: &mut dyn TransactionExecutor,
	administrator: &TenantAdministrator,
	snapshot: &Snapshot,
	scope: &Scope,
	existing: Option<&Budget>,
	strict: bool,
) -> Result<bool> {
	let tenant = administrator.tenant().to_owned();
	let decide = async |tx: &mut dyn TransactionExecutor,
	                    action: &str,
	                    id: &str,
	                    attributes: serde_json::Value|
	       -> Result<bool> {
		if strict {
			administrator
				.require(tx, snapshot, action, id, attributes)
				.await
				.map(|()| true)
				.or_else(|error| match error {
					Error::Forbidden => Ok(false),
					other => Err(other),
				})
		} else {
			administrator
				.allowed(tx, snapshot, action, id, attributes)
				.await
		}
	};
	let subject = administrator.identity.subject.clone();
	let set_by_someone_else = existing.is_some_and(|budget| budget.created_by != subject);
	let scope_right = !set_by_someone_else
		&& match scope {
			Scope::Person { subject: person } => *person == subject,
			Scope::Duty { duty_id } => {
				crate::apps::workspaces::repositories::duties::load(tx, &tenant, *duty_id, false)
					.await?
					.is_some_and(|duty| duty.requester == subject)
			}
			Scope::Node { node } => {
				match crate::apps::identity::repositories::nodes::get_live(tx, &tenant, node).await
				{
					// Judge the Node with its stored `label` and `default`, as
					// the Node management endpoints do.
					Ok(found) => {
						let attributes =
							crate::apps::identity::repositories::nodes::policy_attributes(
								&found.label,
								found.default,
							);
						decide(tx, NODE_ADMINISTER, node, attributes).await?
					}
					Err(Error::NotFound(_)) => false,
					Err(error) => return Err(error),
				}
			}
			Scope::Tenant => false,
		};
	Ok(scope_right || decide(tx, BUDGET_MANAGE, &tenant, json!({})).await?)
}

async fn view(tx: &mut dyn TransactionExecutor, budget: Budget) -> Result<BudgetView> {
	let now = Utc::now();
	let (spent, unpriced) = repository::spent(tx, &budget, now).await?;
	Ok(BudgetView {
		id: budget.id,
		window_start: budget
			.definition
			.period
			.window_start(budget.created_at, now),
		next_reset: budget.definition.period.next_reset(now),
		standing: budget.definition.standing(spent),
		spent_nanocredits: spent,
		unpriced_calls: unpriced,
		definition: budget.definition,
		revision: budget.revision,
		created_at: budget.created_at,
		created_by: budget.created_by,
		updated_at: budget.updated_at,
	})
}

impl TenantAdministrations {
	/// The Budgets this person may set, with their spend.
	pub(crate) async fn budgets(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
	) -> Result<Vec<BudgetView>> {
		let administrator = self.administrator(actor, origin, &tenant).await?;
		let result = self
			.runtime
			.store
			.pool
			.atomic(&administrator.scope()?, async |tx| {
				let snapshot = administrator.lock(tx, false).await?;
				let mut views = vec![];
				for budget in repository::list(tx, &tenant).await? {
					if may_manage(
						tx,
						&administrator,
						&snapshot,
						&budget.definition.scope,
						Some(&budget),
						false,
					)
					.await?
					{
						views.push(view(tx, budget).await?);
					}
				}
				Ok(views)
			})
			.await;
		self.finish(&administrator, result).await
	}

	pub(crate) async fn create_budget(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		definition: Definition,
	) -> Result<BudgetView> {
		definition.validate()?;
		let administrator = self.administrator(actor, origin, &tenant).await?;
		let result = self
			.runtime
			.store
			.pool
			.atomic(&administrator.scope()?, async |tx| {
				let snapshot = administrator.lock(tx, false).await?;
				if !may_manage(tx, &administrator, &snapshot, &definition.scope, None, true).await?
				{
					return Err(Error::Forbidden);
				}
				let budget =
					repository::insert(tx, &tenant, &definition, &administrator.identity.subject)
						.await?;
				view(tx, budget).await
			})
			.await;
		self.finish(&administrator, result).await
	}

	pub(crate) async fn update_budget(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		id: Uuid,
		update: BudgetUpdate,
	) -> Result<BudgetView> {
		update.definition.validate()?;
		let administrator = self.administrator(actor, origin, &tenant).await?;
		let result = self
			.runtime
			.store
			.pool
			.atomic(&administrator.scope()?, async |tx| {
				let snapshot = administrator.lock(tx, false).await?;
				let current = repository::get(tx, &tenant, id, true).await?;
				if !may_manage(
					tx,
					&administrator,
					&snapshot,
					&current.definition.scope,
					Some(&current),
					true,
				)
				.await?
				{
					return Err(Error::Forbidden);
				}
				let budget = repository::update(
					tx,
					&tenant,
					id,
					&update.definition,
					update.expected_revision,
					&administrator.identity.subject,
				)
				.await?;
				view(tx, budget).await
			})
			.await;
		self.finish(&administrator, result).await
	}

	pub(crate) async fn delete_budget(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		id: Uuid,
	) -> Result<()> {
		let administrator = self.administrator(actor, origin, &tenant).await?;
		let result = self
			.runtime
			.store
			.pool
			.atomic(&administrator.scope()?, async |tx| {
				let snapshot = administrator.lock(tx, false).await?;
				let current = repository::get(tx, &tenant, id, true).await?;
				if !may_manage(
					tx,
					&administrator,
					&snapshot,
					&current.definition.scope,
					Some(&current),
					true,
				)
				.await?
				{
					return Err(Error::Forbidden);
				}
				repository::delete(tx, &tenant, id).await
			})
			.await;
		self.finish(&administrator, result).await
	}
}

/// Claims and delivers Soft Limit notices. Each window stays pending until
/// SMTP accepts its message or the Deployment has no SMTP configuration.
pub async fn notice_pass(f: &crate::federation::Federation) -> Result<usize> {
	use repository::NoticeDelivery;
	let now = Utc::now();
	let mut completed = 0;
	for budget in repository::with_soft_limits(&f.store.pool).await? {
		let scope = aidash_application::tenancy::TenancyScope::tenant(&budget.tenant)?;
		let window = budget
			.definition
			.period
			.window_start(budget.created_at, now);
		let notice = f
			.store
			.pool
			.atomic(&scope, async |tx| {
				if let Some(pending) = repository::reclaim_notice(tx, &budget).await? {
					return Ok(Some(pending));
				}
				let (spent, _) = repository::spent(tx, &budget, now).await?;
				if budget.definition.standing(spent) == aidash_domain::budgets::Standing::Under {
					return Ok(None);
				}
				repository::claim_notice(tx, &budget, window, spent).await
			})
			.await?;
		let Some(notice) = notice else {
			continue;
		};
		let delivery = match f
			.config
			.smtp
			.as_ref()
			.filter(|settings| !settings.host.trim().is_empty())
		{
			None => NoticeDelivery::Skipped,
			Some(settings) => match mail(settings, &budget, notice.spent).await {
				Ok(()) => NoticeDelivery::Sent,
				Err(error) => {
					tracing::warn!(%error, budget = %budget.id, "a Budget's Soft Limit mail could not be sent; the next pass tries again");
					NoticeDelivery::Retry
				}
			},
		};
		let handled = !matches!(delivery, NoticeDelivery::Retry);
		if f.store
			.pool
			.atomic(&scope, async |tx| {
				repository::finish_notice(tx, &budget, &notice, delivery).await
			})
			.await? && handled
		{
			completed += 1;
		}
	}
	Ok(completed)
}

async fn mail(
	settings: &crate::apps::operations::serializers::deployment::SmtpSettings,
	budget: &Budget,
	spent: i64,
) -> Result<()> {
	use reinhardt::mail::{EmailMessage, backend_from_settings};
	let mut mail = reinhardt::conf::EmailSettings::default();
	mail.backend = "smtp".into();
	mail.host = settings.host.clone();
	mail.port = settings.port;
	mail.from_email = settings.from.clone();
	mail.username = settings.username.clone();
	mail.password = settings.password.clone();
	mail.use_tls = settings.starttls;
	let backend =
		backend_from_settings(&mail).map_err(|error| Error::External(error.to_string()))?;
	let credits = |nanocredits: i64| format!("{:.2}", nanocredits as f64 / 1e9);
	let scope = match &budget.definition.scope {
		Scope::Tenant => format!("Tenant {}", budget.tenant),
		Scope::Node { node } => format!("Node {node}"),
		Scope::Duty { duty_id } => format!("Duty {duty_id}"),
		Scope::Person { subject } => format!("person {subject}"),
	};
	let body = format!(
		"The Budget of {scope} reached its Soft Limit.\nSpent this window: {} credits\nSoft Limit: {} credits\nHard Limit: {}\n",
		credits(spent),
		budget
			.definition
			.soft_limit_nanocredits
			.map_or("-".into(), credits),
		budget
			.definition
			.hard_limit_nanocredits
			.map_or("none".into(), |hard| format!("{} credits", credits(hard))),
	);
	let message = EmailMessage::builder()
		.subject("Aidash Budget Soft Limit reached")
		.body(body)
		.from(&mail.from_email)
		.to(budget.definition.recipients.clone())
		.build()
		.map_err(|error| Error::External(error.to_string()))?;
	// The entire attempt must finish before the database lease can expire.
	tokio::time::timeout(
		std::time::Duration::from_secs(30),
		backend.send_messages(&[message]),
	)
	.await
	.map_err(|error| Error::External(error.to_string()))?
	.map_err(|error| Error::External(error.to_string()))?;
	Ok(())
}
