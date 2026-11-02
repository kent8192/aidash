// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Declarative Authentication Pages. Sensitive inputs stay in mounted hooks.
use crate::{
	app::{Action, App, Factor, State},
	model::{Account, Enrollment, LoginContext, Phase},
};
use reinhardt::pages::{Page, page};
use std::rc::Rc;

fn notice(message: String, error: bool) -> Page {
	page!({
		div {
			class: if error { "notice error" } else { "notice" },
			role: if error { "alert" } else { "status" },
			{ message }
		}
	})
}
fn button(
	app: Rc<App>,
	label: impl Into<String>,
	class: &'static str,
	disabled: bool,
	action: Action,
) -> Page {
	let label = label.into();
	page!({
		button {
			type: "button",
			class: class,
			disabled: disabled,
			@click: move |_| app.dispatch(action.clone()),
			{ label }
		}
	})
}

pub fn frame(content: Page) -> Page {
	page!({
		div {
			class: "layout",
			header {
				class: "brand",
				span {
					class: "brand-mark",
					aria_hidden: "true",
					"a"
				}
				strong { "aidash" }
				span {
					class: "brand-divider"
				}
				span { "account" }
			}
			main { { content } }
			footer { "One Aidash Account. Your product permissions stay with each product." }
		}
	})
}

pub fn content(app: Rc<App>, state: State) -> Page {
	if state.signed_out {
		return page!({
			span {
				class: "eyebrow",
				"Aidash Account"
			}
			h1 { "Signed out everywhere" }
			p {
				class: "intro",
				"Sign in again from Cloud or Marketplace when you are ready. Work already admitted continues under its original authority."
			}
		});
	}
	if state.loading {
		return page!({
			p {
				class: "loading",
				role: "status",
				"Preparing a secure sign-in…"
			}
		});
	}
	if app.account_page {
		return match state.account.clone() {
			Some(account) => account_settings(app, state, account),
			None => page!({
				h1 { "Your Aidash Account" }
				{ failure(app.clone(), state.clone()) }
				p {
					class: "small muted",
					"If your session has ended, start sign-in from Cloud or Marketplace."
				}
			}),
		};
	}
	if let Some(context) = state.context.clone() {
		return sign_in(app, state, context);
	}
	if state.error.is_some() {
		return failure(app, state);
	}
	page!({
		span {
			class: "eyebrow",
			"Aidash Account"
		}
		h1 { "Start with your product" }
		p {
			class: "intro",
			"Start sign-in from Cloud or Marketplace so we can safely return you to the right place."
		}
	})
}

fn failure(app: Rc<App>, state: State) -> Page {
	page!({
		{
			notice(
				state
					.error
					.clone()
					.unwrap_or_else(|| "Start sign-in again from your product.".into()),
				true,
			)
		}
		{
			button(
				app.clone(),
				"Try again",
				"secondary",
				state.busy,
				Action::Restart,
			)
		}
	})
}

fn sign_in(app: Rc<App>, state: State, context: LoginContext) -> Page {
	page!({
		span {
			class: "eyebrow",
			{ format!("Aidash Account · {}", context.client_id) }
		}
		h1 { { context.phase.title() } }
		p {
			class: "intro",
			{ context.phase.description() }
		}
		if context.phase == Phase::ChooseAccount {
			{ state
			.error
			.clone()
			.map(|error| notice(error, true))
			.unwrap_or(Page::Empty) }
			div {
				class: "choices",
				if context.relink_only { { notice("This sign-in method retains its original Aidash Account association. Prove that original Account to reattach it.".into(), false) } }
				else { { button(
					app.clone(),
					"Create a new Aidash Account",
					"",
					state.busy,
					Action::Choose("new"),
				) } } { button(
						app.clone(),
						"Use an existing Aidash Account",
						"secondary",
						state.busy,
						Action::Choose("existing"),
					) }
				p {
					class: "small muted",
					"Accounts are never linked by matching email addresses. Existing Accounts are not merged."
				}
			}
		} else { { sign_in_form(app.clone(), state.clone(), context.clone()) } }
	})
}

fn sign_in_form(app: Rc<App>, state: State, context: LoginContext) -> Page {
	let tenant_options = context
		.tenant_ids
		.iter()
		.map(|tenant| {
			let tenant = tenant.clone();
			let selected = tenant == context.selected_tenant_id;
			page!({
				option {
					value: tenant.clone(),
					selected: selected,
					{ tenant }
				}
			})
		})
		.collect::<Vec<_>>();
	let providers = context
		.providers
		.iter()
		.filter(|id| id.as_str() != "password")
		.map(|provider| {
			let label = if provider == "google.com" {
				"Continue with Google".into()
			} else {
				format!("Continue with {provider}")
			};
			button(
				app.clone(),
				label,
				"secondary",
				!state.ready || state.busy,
				Action::Popup(provider.clone()),
			)
		})
		.collect::<Vec<_>>();
	let password_enabled = context
		.providers
		.iter()
		.any(|provider| provider == "password");
	let fields = app.credentials.fields.clone();
	let validate = app.credentials.validate.clone();
	page!({
		if context.tenant_ids.len() > 1 && state.factor.is_none() {
			div {
				class: "field",
				label {
					for: "signin-directory",
					"Sign-in directory"
				}
				select {
					id: "signin-directory",
					disabled: state.busy || !state.ready,
					@change: move |event| {
						if let Ok(tenant) = event.value() {
							app.dispatch(Action::Tenant(tenant));
						}
					},
					{ Page::fragment(tenant_options.clone()) }
				}
			}
		}
		{ state
		.error
		.clone()
		.map(|error| notice(error, true))
		.unwrap_or(Page::Empty) }
		{ state
		.notice
		.clone()
		.map(|message| notice(message, false))
		.unwrap_or(Page::Empty) }
		if state.factor.is_some() { { factor_form(app.clone(), state.clone()) } } else {
			div {
				class: "providers",
				{ Page::fragment(providers.clone()) }
			}
			{ state
			.redirect_provider
			.clone()
			.map(|provider| {
				button(
					app.clone(),
					"Continue in this tab",
					"secondary",
					state.busy,
					Action::Redirect(provider),
				)
			})
			.unwrap_or(Page::Empty) }
			if password_enabled {
				div {
					class: "divider",
					span { "or use email" }
				}
				form {
					@submit: move |event| {
						event.prevent_default();
						if (validate)() {
							app.dispatch(Action::Password);
						}
					},
					{ (fields)(state.busy) }
					button {
						type: "submit",
						class: "full-width",
						disabled: !state.ready || state.busy,
						{
							if state.busy { "Verifying…" } else { "Sign in with email" }
						}
					}
				}
				div {
					class: "email-tools",
					if context.password_sign_up && context.phase == Phase::SignIn { { email_tool(
						app.clone(),
						state.clone(),
						"Register a login",
						Action::Register,
					) } }
					{ email_tool(
						app.clone(),
						state.clone(),
						"Resend verification email",
						Action::Resend,
					) }
				}
			}
			if context.providers.is_empty() { { notice(
				"No sign-in methods are configured for this directory. Contact your administrator.".into(),
				true,
			) } }
			if !state.ready && state.error.is_some() { { button(
				app.clone(),
				"Start this step again",
				"secondary",
				state.busy,
				Action::Restart,
			) } }
		}
	})
}

fn factor_form(app: Rc<App>, state: State) -> Page {
	let enroll = state.factor == Some(Factor::Enroll);
	let setup = state
		.enrollment
		.clone()
		.map(|enrollment| enrollment_view(app.clone(), enrollment))
		.unwrap_or(Page::Empty);
	let field = app.code.field.clone();
	let validate = app.code.validate.clone();
	page!({
		section {
			h2 { {
				if enroll { "Set up two-step verification" } else { "Two-step verification" }
			} }
			p {
				class: "muted",
				{
					if enroll { "Add this key to your authenticator app, then enter the code it shows." } else { "Enter the code from your authenticator app to finish this proof." }
				}
			}
			{ setup }
			form {
				@submit: move |event| {
					event.prevent_default();
					if (validate)() {
						app.dispatch(Action::Code);
					}
				},
				{ (field)() }
				button {
					type: "submit",
					disabled: state.busy || (enroll && state.enrollment.is_none()),
					{
						if state.busy { "Verifying…" } else { "Verify code" }
					}
				}
			}
			{ button(
				app.clone(),
				"Start this step again",
				"text-button",
				state.busy,
				Action::Restart,
			) }
		}
	})
}

fn email_tool(app: Rc<App>, state: State, label: &'static str, action: Action) -> Page {
	page!({
		button {
			type: "button",
			class: "text-button",
			disabled: state.busy || !state.ready || !(app.credentials.complete)(),
			@click: move |_| app.dispatch(action.clone()),
			{ label }
		}
	})
}

fn account_settings(app: Rc<App>, state: State, account: Account) -> Page {
	let count = account
		.identities
		.iter()
		.filter(|identity| identity.linked)
		.count();
	let identities = account
		.identities
		.iter()
		.map(|identity| {
			let identity = identity.clone();
			let current = identity.id == account.source_identity_id;
			let app = app.clone();
			let busy = state.busy;
			page!({
				article {
					class: "identity",
					div {
						class: "identity-heading",
						strong { {
							if identity.linked { "Linked" } else { "Unlinked" }
						} }
						if current {
							span {
								class: "badge",
								"Current session"
							}
						}
					}
					dl {
						dt { "Issuer" }
						dd { { identity.issuer } }
						dt { "Subject" }
						dd {
							code { { identity.subject } }
						}
					}
					if identity.linked {
						{ button(
							app.clone(),
							"Unlink",
							"secondary compact",
							busy || count < 2,
							Action::Unlink(identity.id.clone()),
						) }
						if count < 2 {
							p {
								class: "small muted",
								"Keep at least one usable sign-in method."
							}
						}
					} else {
						p {
							class: "small muted",
							"The original Account association is retained. Add this method again using fresh proof."
						}
					}
				}
			})
		})
		.collect::<Vec<_>>();
	page!({
		span {
			class: "eyebrow",
			"Account settings"
		}
		h1 { "Sign-in methods" }
		p {
			class: "intro",
			"Changes require fresh proof. Linking does not combine product permissions or transfer ownership."
		}
		{ state
		.error
		.clone()
		.map(|error| notice(error, true))
		.unwrap_or(Page::Empty) }
		div {
			class: "account-reference",
			span { "Aidash Account" }
			code { { account.account_id } }
		}
		div {
			class: "identity-list",
			{ Page::fragment(identities.clone()) }
		}
		{ button(
			app.clone(),
			"Add a sign-in method",
			"",
			state.busy,
			Action::Link,
		) }
		p {
			class: "small muted",
			"Recover lost access through your identity provider. This page cannot replace its recovery checks."
		}
		section {
			class: "global-logout",
			h2 { "Sign out everywhere" }
			p {
				class: "small muted",
				"End sign-in sessions across all products and devices. Work already admitted continues."
			}
			{ button(
				app.clone(),
				"Sign out of all products and devices",
				"secondary",
				state.busy,
				Action::LogoutAll,
			) }
		}
	})
}

fn enrollment_view(app: Rc<App>, enrollment: Enrollment) -> Page {
	page!({
		div {
			class: "setup-key",
			span { "Setup key" }
			code { { enrollment.secret } }
			if enrollment.uri.starts_with("otpauth://") {
				a {
					href: "#",
					@click: move |event| {
						event.prevent_default();
						app.dispatch(Action::Authenticator(enrollment.uri.clone()));
					},
					"Open authenticator app"
				}
			}
		}
	})
}
