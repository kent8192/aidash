//! Calls of remote Runs that a Federation tool list stopped, and the answers of the people who
//! hold the list (ADR 0139). The receiving Node's people answer for its Allowance and Block, the
//! Home's for its Request; each needs `node.federation_tools.manage` on the registration, and the
//! asked-about side is told by the Run's pause itself, never guessed.
use super::{NodeRegistrations, attributes, require_node};
use crate::apps::identity::models::{
	AdministrationActor, AdministrationEvent, TenantAdministrator,
};
use crate::apps::identity::oidc::BrowserOrigin;
use crate::apps::identity::repositories::{node_registrations as records, tool_asks as asks};
use crate::apps::identity::serializers::tool_asks::{
	ToolAskAnswerInput, ToolAskAnswered, ToolAskRecord, ToolAsks,
};
use crate::authorization::identity::Actor;
use crate::{Error, Result};
use aidash_domain::RunControlAction;
use aidash_domain::federation::execution::admission::{Activation, RemoteExecutionControlState};
use aidash_domain::federation::tool_lists::{
	AskChoice, AskSide, FederationToolRequest, ToolPattern, answered_pattern,
};
use aidash_domain::identity::tenant_administration as rules;
use aidash_domain::nodes::{NodeAddress, NodeName};
use reinhardt::db::backends::TransactionExecutor;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// Home asks answered from one request: more would take longer than a page is worth waiting for.
const HOME_ASKS: usize = 25;
/// How long the receiving Node may take to say whether a Run waits, before the Home skips it.
const STATUS_TIMEOUT: Duration = Duration::from_secs(2);

/// Which list of the registration an answer extends.
#[derive(Clone, Copy)]
enum Registered {
	/// The Home's Request.
	Request,
	/// The receiving Node's Allowance.
	Allowance,
	/// The receiving Node's Block.
	Block,
}

/// What an answer is about, found from its path id.
enum Found {
	Receiver(Box<asks::ReceiverAsk>),
	Home(asks::HomeGrant),
}

fn parse_address(source_node: &str) -> Result<NodeAddress> {
	// The address holds `/` and `:`, so a client sends it percent-encoded.
	let source_node = percent_encoding::percent_decode_str(source_node)
		.decode_utf8()
		.map_err(|_| {
			Error::Invalid("a Node address is aidash://<deployment>/<node name>".into())
		})?;
	Ok(NodeAddress::parse(&source_node)?)
}

/// Record the answer as the tenant administration audit event of a list change (decision 29).
async fn audit(
	tx: &mut reinhardt::db::orm::AtomicTransaction,
	by: &AdministrationActor,
	tenant: &str,
) -> Result<()> {
	by.record(
		tx,
		AdministrationEvent {
			action: rules::NODE_FEDERATION_TOOLS_MANAGE,
			identity: None,
			registration: None,
			mapping: None,
			tenant: Some(tenant),
			subject: None,
			invitation: None,
		},
	)
	.await
}

/// Add `pattern` to one list of the registration, as an ordinary list change: a new revision
/// and a history row.
async fn extend_registered_list(
	tx: &mut dyn TransactionExecutor,
	registration: (&str, &str, &NodeAddress),
	list: Registered,
	pattern: ToolPattern,
	actor: &str,
) -> Result<()> {
	let (tenant, node, source) = registration;
	let current = records::get(tx, tenant, node, source)
		.await?
		.ok_or_else(|| Error::Conflict("node registration revision changed".into()))?;
	let (request, allowance, block) = (
		current.tool_request.clone(),
		current.tool_allowance.clone(),
		current.tool_block.clone(),
	);
	let (request, allowance, block) = match list {
		Registered::Request => (
			FederationToolRequest {
				tools: request.tools.with(pattern)?,
				hosts: request.hosts,
			},
			allowance,
			block,
		),
		Registered::Allowance => (request, allowance.with(pattern)?, block),
		Registered::Block => (request, allowance, block.with(pattern)?),
	};
	let stored = records::update_tool_lists(
		tx,
		tenant,
		node,
		source,
		(&request, &allowance, &block),
		actor,
		current.revision,
	)
	.await?
	.ok_or_else(|| Error::Conflict("node registration revision changed".into()))?;
	records::append_history(tx, &stored).await
}

impl NodeRegistrations {
	/// The calls waiting for the people who hold the lists of the registration at `node` of
	/// `source_node`: remote Runs this Node hosts for that Home, and Remote Grants this Node issued
	/// to that receiving Node. Only holders of `node.federation_tools.manage` read them.
	pub(crate) async fn tool_asks(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		node: String,
		source_node: String,
	) -> Result<ToolAsks> {
		let node = NodeName::parse(&node)?;
		let source = parse_address(&source_node)?;
		let f = self.runtime.clone();
		let (hosted, issued) = self
			.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				f.store
					.pool
					.atomic(&administrator.scope()?, async |tx| {
						let snapshot = administrator.lock(tx, false).await?;
						administrator
							.require(
								tx,
								&snapshot,
								rules::NODE_FEDERATION_TOOLS_MANAGE,
								&source.to_string(),
								attributes(node.as_str(), &source),
							)
							.await?;
						require_node(tx, &tenant, &node).await?;
						let hosted =
							asks::receiver_asks(tx, &tenant, node.as_str(), &source, None).await?;
						let mut issued = Vec::new();
						for grant in asks::home_grants(tx, &tenant, node.as_str(), &source, None)
							.await?
							.into_iter()
							.take(HOME_ASKS)
						{
							let title = asks::task_title(tx, grant.task_id).await?;
							issued.push((grant, title));
						}
						Ok((hosted, issued))
					})
					.await
			})
			.await?;
		let mut records: Vec<ToolAskRecord> = hosted
			.into_iter()
			.map(|ask| ToolAskRecord {
				side: AskSide::Receiver,
				id: ask.run,
				run_id: ask.run,
				grant_id: ask.grant_id,
				task_id: ask.task_id,
				task_title: ask.task_title,
				tool: ask.paused.tool,
				key: ask.ask.key,
				paused_since: Some(ask.paused_since),
			})
			.collect();
		// The Home learns a remote Run's pause only by asking the receiving Node.
		let waiting = futures_util::future::join_all(issued.into_iter().map(|(grant, title)| {
			let (tenant, node, source) = (&tenant, &node, &source);
			async move {
				let activation = self.home_status(tenant, node, source, grant.id).await?;
				let call = activation.declinable_call?;
				let key = call.home_list_key?;
				Some(ToolAskRecord {
					side: AskSide::Home,
					id: grant.id,
					run_id: activation.run_id,
					grant_id: grant.id,
					task_id: grant.task_id,
					task_title: title,
					tool: call.tool,
					key,
					paused_since: None,
				})
			}
		}))
		.await;
		records.extend(waiting.into_iter().flatten());
		Ok(ToolAsks { asks: records })
	}

	/// The paused remote Run of `grant`, if the receiving Node answers in time and the Run still
	/// waits. The Home calls as its Node, as for any status of its Remote Grants.
	async fn home_status(
		&self,
		tenant: &str,
		node: &NodeName,
		receiver: &NodeAddress,
		grant: Uuid,
	) -> Option<Activation> {
		let sender = aidash_domain::federation::route::Sender {
			node: Some(node.clone()),
			..crate::authorization::peer::node_sender(tenant).ok()?
		};
		let status: Result<Option<Activation>> = tokio::time::timeout(
			STATUS_TIMEOUT,
			crate::authorization::peer::authority_request_keeping_not_found(
				&self.runtime,
				&sender,
				&receiver.to_string(),
				"/scoped/execution/status",
				&json!({"grant_id": grant}),
			),
		)
		.await
		.ok()?;
		status.ok().flatten().filter(|activation| {
			matches!(activation.control, RemoteExecutionControlState::Paused)
				&& activation.recoverable()
		})
	}

	/// Answer one waiting call. `id` is the admission of a receiver ask or the grant of a Home ask.
	/// Allowing the call for this Run or adding it resumes the Run; if the other side still lacks
	/// the tool the Run pauses again and that side is asked, so an answer never loops.
	#[allow(clippy::too_many_arguments)]
	pub(crate) async fn answer_tool_ask(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		node: String,
		source_node: String,
		id: Uuid,
		input: ToolAskAnswerInput,
	) -> Result<ToolAskAnswered> {
		let node = NodeName::parse(&node)?;
		let source = parse_address(&source_node)?;
		let f = self.runtime.clone();
		self.administrations()
			.run(actor, origin, &tenant, async |administrator| {
				let found = f
					.store
					.pool
					.atomic(&administrator.scope()?, async |tx| {
						let snapshot = administrator.lock(tx, false).await?;
						administrator
							.require(
								tx,
								&snapshot,
								rules::NODE_FEDERATION_TOOLS_MANAGE,
								&source.to_string(),
								attributes(node.as_str(), &source),
							)
							.await?;
						require_node(tx, &tenant, &node).await?;
						if let Some(ask) =
							asks::receiver_asks(tx, &tenant, node.as_str(), &source, Some(id))
								.await?
								.into_iter()
								.next()
						{
							return Ok(Found::Receiver(Box::new(ask)));
						}
						if let Some(grant) =
							asks::home_grants(tx, &tenant, node.as_str(), &source, Some(id))
								.await?
								.into_iter()
								.next()
						{
							return Ok(Found::Home(grant));
						}
						Err(Error::NotFound("no call waits for an answer there".into()))
					})
					.await?;
				match found {
					Found::Receiver(ask) => {
						self.answer_as_receiver(
							administrator,
							&tenant,
							&node,
							&source,
							*ask,
							&input,
						)
						.await
					}
					Found::Home(grant) => {
						self.answer_as_home(administrator, &tenant, &node, &source, grant, &input)
							.await
					}
				}
			})
			.await
	}

	/// The receiving Node's people answer for a remote Run it hosts for `source`.
	async fn answer_as_receiver(
		&self,
		administrator: &Arc<TenantAdministrator>,
		tenant: &str,
		node: &NodeName,
		source: &NodeAddress,
		ask: asks::ReceiverAsk,
		input: &ToolAskAnswerInput,
	) -> Result<ToolAskAnswered> {
		let f = &self.runtime;
		let key = ask.ask.key.clone();
		let by = AdministrationActor::TenantAdministrator(administrator.clone());
		let actor = format!("oidc:{}", administrator.identity_id);
		let mut listed = None;
		match input.choice {
			AskChoice::Decline => {
				// The receiving Node's people decline their own list's pause as the Home's people
				// decline any other: the call is answered with a Declined Call and the Run goes on.
				let store = &f.store;
				let scope = store.run_tenancy(ask.run).await?;
				let mut tx = store.pool.begin(&scope).await?;
				crate::transactions::gate::read_in(&mut tx).await?;
				let declined = store
					.decline_call_in(
						&mut tx,
						ask.run,
						&ask.paused.key,
						&administrator.identity.subject,
						false,
					)
					.await?;
				tx.commit().await?;
				store.run_control_committed(ask.run, declined.run.control);
				f.store
					.pool
					.atomic(&administrator.scope()?, async |tx| {
						crate::apps::execution::models::event_records::append_run(
							tx,
							ask.run,
							None,
							"run.tool_ask_answered",
							json!({"run_id": ask.run, "side": "receiver", "choice": "decline", "key": key, "by": administrator.identity.subject}),
						)
						.await?;
						audit(tx, &by, tenant).await
					})
					.await?;
			}
			choice => {
				let pattern = answered_pattern(&key, input.pattern.as_deref())?;
				listed = Some(pattern.as_str().to_owned());
				f.store
					.pool
					.atomic(&administrator.scope()?, async |tx| {
						match choice {
							AskChoice::RunOnly => {
								asks::allow_run_only_on_receiver(tx, ask.run, pattern.clone())
									.await?;
							}
							AskChoice::Add => {
								extend_registered_list(
									tx,
									(tenant, node.as_str(), source),
									Registered::Allowance,
									pattern.clone(),
									&actor,
								)
								.await?;
							}
							_ => {
								extend_registered_list(
									tx,
									(tenant, node.as_str(), source),
									Registered::Block,
									pattern.clone(),
									&actor,
								)
								.await?;
							}
						}
						// The pause's own call is answered by the next attempt: allowed, it runs;
						// blocked, it answers the same words as an unknown tool.
						f.store
							.control_in_executor(tx, ask.run, RunControlAction::Resume)
							.await?;
						crate::apps::execution::models::event_records::append_run(
							tx,
							ask.run,
							None,
							"run.tool_ask_answered",
							json!({"run_id": ask.run, "side": "receiver", "choice": input.choice, "key": key, "pattern": pattern.as_str(), "by": administrator.identity.subject}),
						)
						.await?;
						audit(tx, &by, tenant).await
					})
					.await?;
			}
		}
		f.notify.notify_waiters();
		tracing::info!(run_id = %ask.run, tool = %ask.paused.tool, choice = ?input.choice, "receiver answered a tool ask");
		Ok(ToolAskAnswered {
			side: AskSide::Receiver,
			choice: input.choice,
			key,
			pattern: listed,
			resumed: true,
		})
	}

	/// The Home's people answer for a Remote Grant it issued to `receiver`.
	async fn answer_as_home(
		&self,
		administrator: &Arc<TenantAdministrator>,
		tenant: &str,
		node: &NodeName,
		receiver: &NodeAddress,
		grant: asks::HomeGrant,
		input: &ToolAskAnswerInput,
	) -> Result<ToolAskAnswered> {
		if !input.choice.allowed_on(AskSide::Home) {
			return Err(Error::Unprocessable(
				"only the receiving Node keeps a Block".into(),
			));
		}
		let f = &self.runtime;
		// The receiving Node says which call waits for the Home; the Home never guesses it.
		let activation = self
			.home_status(tenant, node, receiver, grant.id)
			.await
			.ok_or_else(|| Error::Conflict("the remote Run does not wait for an answer".into()))?;
		let call = activation
			.declinable_call
			.ok_or_else(|| Error::Conflict("the remote Run does not wait for an answer".into()))?;
		let key = call.home_list_key.clone().ok_or_else(|| {
			Error::Conflict("the remote Run waits for the receiving Node, not the Home".into())
		})?;
		let by = AdministrationActor::TenantAdministrator(administrator.clone());
		let actor = format!("oidc:{}", administrator.identity_id);
		let repository = crate::bootstrap::home_execution_repository(
			f,
			Actor::Subject(administrator.identity.clone()),
		);
		let mut listed = None;
		match input.choice {
			AskChoice::Decline => {
				aidash_application::authorization::home::decline_tool_ask(
					&repository,
					grant.task_id,
					grant.id,
					&call.call,
				)
				.await
				.map_err(Error::from)?;
				f.store
					.pool
					.atomic(&administrator.scope()?, async |tx| {
						audit(tx, &by, tenant).await
					})
					.await?;
			}
			choice => {
				let pattern = answered_pattern(&key, input.pattern.as_deref())?;
				listed = Some(pattern.as_str().to_owned());
				f.store
					.pool
					.atomic(&administrator.scope()?, async |tx| {
						if matches!(choice, AskChoice::RunOnly) {
							asks::allow_run_only_on_home(tx, grant.id, pattern.clone()).await?;
						} else {
							extend_registered_list(
								tx,
								(tenant, node.as_str(), receiver),
								Registered::Request,
								pattern.clone(),
								&actor,
							)
							.await?;
						}
						audit(tx, &by, tenant).await
					})
					.await?;
				// The list is saved before the Run is told, so its next describe carries it.
				aidash_application::authorization::home::resume_after_tool_answer(
					&repository,
					grant.task_id,
					grant.id,
				)
				.await
				.map_err(Error::from)?;
			}
		}
		tracing::info!(grant_id = %grant.id, tool = %call.tool, choice = ?input.choice, "home answered a tool ask");
		Ok(ToolAskAnswered {
			side: AskSide::Home,
			choice: input.choice,
			key,
			pattern: listed,
			resumed: true,
		})
	}
}
