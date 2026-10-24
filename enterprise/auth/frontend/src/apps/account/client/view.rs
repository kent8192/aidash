//! Real Reinhardt Page/Element trees. Credentials use fine-grained Rust controls.
use crate::{
	app::{Action, App, Factor, State},
	model::{Account, LoginContext, Phase},
};
use reinhardt::pages::{IntoPage, Page, PageElement, component::ControlBinding};
use std::rc::Rc;
use wasm_bindgen::JsCast;

fn element(tag: &'static str, class: &'static str, children: Vec<Page>) -> Page {
	PageElement::new(tag)
		.attr("class", class)
		.children(children)
		.into_page()
}
fn text(tag: &'static str, class: &'static str, value: impl Into<String>) -> Page {
	element(tag, class, vec![Page::text(value.into())])
}
fn fragment(children: Vec<Page>) -> Page {
	Page::Fragment(children)
}
fn notice(message: impl Into<String>, error: bool) -> Page {
	PageElement::new("div")
		.attr("class", if error { "notice error" } else { "notice" })
		.attr("role", if error { "alert" } else { "status" })
		.child(Page::text(message.into()))
		.into_page()
}
fn button(
	app: &Rc<App>,
	label: &str,
	class: &'static str,
	disabled: bool,
	action: Action,
) -> PageElement {
	let app = app.clone();
	PageElement::new("button")
		.attr("type", "button")
		.attr("class", class)
		.attr("disabled", if disabled { "true" } else { "false" })
		.child(Page::text(label.to_owned()))
		.listener("click", move |_| app.dispatch(action.clone()))
}
fn submit(label: &str, disabled: bool, class: &'static str) -> Page {
	PageElement::new("button")
		.attr("type", "submit")
		.attr("class", class)
		.attr("disabled", if disabled { "true" } else { "false" })
		.child(Page::text(label.to_owned()))
		.into_page()
}
fn form(app: &Rc<App>, action: Action, children: Vec<Page>) -> Page {
	let app = app.clone();
	PageElement::new("form")
		.children(children)
		.listener("submit", move |event| {
			event.prevent_default();
			app.dispatch(action.clone());
		})
		.into_page()
}

pub fn frame(content: Page) -> Page {
	element(
		"div",
		"layout",
		vec![
			element(
				"header",
				"brand",
				vec![
					PageElement::new("span")
						.attr("class", "brand-mark")
						.attr("aria-hidden", "true")
						.child("a")
						.into_page(),
					text("strong", "", "aidash"),
					element("span", "brand-divider", vec![]),
					text("span", "", "account"),
				],
			),
			element("main", "", vec![content]),
			text(
				"footer",
				"",
				"One Aidash Account. Your product permissions stay with each product.",
			),
		],
	)
}
pub fn content(app: &Rc<App>, state: &State) -> Page {
	if state.signed_out {
		return fragment(vec![
			text("span", "eyebrow", "Aidash Account"),
			text("h1", "", "Signed out everywhere"),
			text(
				"p",
				"intro",
				"Sign in again from Cloud or Marketplace when you are ready. Work already admitted continues under its original authority.",
			),
		]);
	}
	if state.loading {
		return PageElement::new("p")
			.attr("class", "loading")
			.attr("role", "status")
			.child("Preparing a secure sign-in…")
			.into_page();
	}
	if app.account_page {
		return state
			.account
			.as_ref()
			.map(|account| account_settings(app, state, account))
			.unwrap_or_else(|| {
				fragment(vec![
					text("h1", "", "Your Aidash Account"),
					failure(app, state),
					text(
						"p",
						"small muted",
						"If your session has ended, start sign-in from Cloud or Marketplace.",
					),
				])
			});
	}
	if let Some(context) = &state.context {
		return sign_in(app, state, context);
	}
	if state.error.is_some() {
		return failure(app, state);
	}
	fragment(vec![
		text("span", "eyebrow", "Aidash Account"),
		text("h1", "", "Start with your product"),
		text(
			"p",
			"intro",
			"Start sign-in from Cloud or Marketplace so we can safely return you to the right place.",
		),
	])
}
fn failure(app: &Rc<App>, state: &State) -> Page {
	fragment(vec![
		notice(
			state
				.error
				.as_deref()
				.unwrap_or("Start sign-in again from your product."),
			true,
		),
		button(app, "Try again", "secondary", state.busy, Action::Restart).into_page(),
	])
}
fn sign_in(app: &Rc<App>, state: &State, context: &LoginContext) -> Page {
	let mut children = vec![
		text(
			"span",
			"eyebrow",
			format!("Aidash Account · {}", context.client_id),
		),
		text("h1", "", context.phase.title()),
		text("p", "intro", context.phase.description()),
	];
	if context.phase == Phase::ChooseAccount {
		if let Some(error) = &state.error {
			children.push(notice(error, true));
		}
		let mut choices = vec![];
		if context.relink_only {
			choices.push(notice("This sign-in method retains its original Aidash Account association. Prove that original Account to reattach it.",false));
		} else {
			choices.push(
				button(
					app,
					"Create a new Aidash Account",
					"",
					state.busy,
					Action::Choose("new"),
				)
				.into_page(),
			);
		}
		choices.push(
			button(
				app,
				"Use an existing Aidash Account",
				"secondary",
				state.busy,
				Action::Choose("existing"),
			)
			.into_page(),
		);
		choices.push(text(
			"p",
			"small muted",
			"Accounts are never linked by matching email addresses. Existing Accounts are not merged.",
		));
		children.push(element("div", "choices", choices));
	} else {
		children.push(sign_in_form(app, state, context));
	}
	fragment(children)
}

fn sign_in_form(app: &Rc<App>, state: &State, context: &LoginContext) -> Page {
	let mut children = vec![];
	if context.tenant_ids.len() > 1 && state.factor.is_none() {
		let controller = app.clone();
		let options = context
			.tenant_ids
			.iter()
			.map(|tenant| {
				PageElement::new("option")
					.attr("value", tenant.clone())
					.attr(
						"selected",
						if *tenant == context.selected_tenant_id {
							"true"
						} else {
							"false"
						},
					)
					.child(Page::text(tenant.clone()))
					.into_page()
			})
			.collect::<Vec<_>>();
		let select = PageElement::new("select")
			.attr(
				"disabled",
				if state.busy || !state.ready {
					"true"
				} else {
					"false"
				},
			)
			.children(options)
			.listener("change", move |event| {
				if let Some(select) = event
					.target()
					.and_then(|target| target.dyn_into::<web_sys::HtmlSelectElement>().ok())
				{
					controller.dispatch(Action::Tenant(select.value()));
				}
			})
			.into_page();
		children.push(element(
			"label",
			"field",
			vec![Page::text("Sign-in directory"), select],
		));
	}
	if let Some(error) = &state.error {
		children.push(notice(error, true));
	}
	if let Some(message) = &state.notice {
		children.push(notice(message, false));
	}
	if let Some(factor) = state.factor {
		let mut section = vec![
			text(
				"h2",
				"",
				if factor == Factor::Enroll {
					"Set up two-step verification"
				} else {
					"Two-step verification"
				},
			),
			text(
				"p",
				"muted",
				if factor == Factor::Enroll {
					"Add this key to your authenticator app, then enter the code it shows."
				} else {
					"Enter the code from your authenticator app to finish this proof."
				},
			),
		];
		if let Some(enrollment) = &state.enrollment {
			let mut key = vec![
				text("span", "", "Setup key"),
				text("code", "", &enrollment.secret),
			];
			if enrollment.uri.starts_with("otpauth://") {
				let app = app.clone();
				let uri = enrollment.uri.clone();
				key.push(
					PageElement::new("a")
						.attr("href", "#")
						.child("Open authenticator app")
						.listener("click", move |event| {
							event.prevent_default();
							app.dispatch(Action::Authenticator(uri.clone()));
						})
						.into_page(),
				);
			}
			section.push(element("div", "setup-key", key));
		}
		let code = PageElement::new("input")
			.attr("name", "code")
			.attr("inputmode", "numeric")
			.attr("autocomplete", "one-time-code")
			.attr("pattern", "[0-9]{6}")
			.attr("maxlength", "6")
			.attr("required", "true")
			.control_binding(ControlBinding::text(app.code))
			.into_page();
		section.push(form(
			app,
			Action::Code,
			vec![
				element(
					"label",
					"field",
					vec![Page::text("Verification code"), code],
				),
				submit(
					if state.busy {
						"Verifying…"
					} else {
						"Verify code"
					},
					state.busy || (factor == Factor::Enroll && state.enrollment.is_none()),
					"",
				),
			],
		));
		section.push(
			button(
				app,
				"Start this step again",
				"text-button",
				state.busy,
				Action::Restart,
			)
			.into_page(),
		);
		children.push(element("section", "", section));
		return fragment(children);
	}
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
				app,
				&label,
				"secondary",
				!state.ready || state.busy,
				Action::Popup(provider.clone()),
			)
			.into_page()
		})
		.collect();
	children.push(element("div", "providers", providers));
	if let Some(provider) = &state.redirect_provider {
		children.push(
			button(
				app,
				"Continue in this tab",
				"secondary",
				state.busy,
				Action::Redirect(provider.clone()),
			)
			.into_page(),
		);
	}
	if context
		.providers
		.iter()
		.any(|provider| provider == "password")
	{
		children.push(element(
			"div",
			"divider",
			vec![text("span", "", "or use email")],
		));
		let email = PageElement::new("input")
			.attr("type", "email")
			.attr("autocomplete", "username")
			.attr("required", "true")
			.attr("disabled", if state.busy { "true" } else { "false" })
			.control_binding(ControlBinding::text(app.email))
			.into_page();
		let password = PageElement::new("input")
			.attr("type", "password")
			.attr("autocomplete", "current-password")
			.attr("required", "true")
			.attr("disabled", if state.busy { "true" } else { "false" })
			.control_binding(ControlBinding::text(app.password))
			.into_page();
		children.push(form(
			app,
			Action::Password,
			vec![
				element("label", "field", vec![Page::text("Email"), email]),
				element("label", "field", vec![Page::text("Password"), password]),
				submit(
					if state.busy {
						"Verifying…"
					} else {
						"Sign in with email"
					},
					!state.ready || state.busy,
					"full-width",
				),
			],
		));
		let mut tools = vec![];
		if context.password_sign_up && context.phase == Phase::SignIn {
			tools.push(email_tool(app, state, "Register a login", Action::Register));
		}
		tools.push(email_tool(
			app,
			state,
			"Resend verification email",
			Action::Resend,
		));
		children.push(element("div", "email-tools", tools));
	}
	if context.providers.is_empty() {
		children.push(notice(
			"No sign-in methods are configured for this directory. Contact your administrator.",
			true,
		));
	}
	if !state.ready && state.error.is_some() {
		children.push(
			button(
				app,
				"Start this step again",
				"secondary",
				state.busy,
				Action::Restart,
			)
			.into_page(),
		);
	}
	fragment(children)
}

fn email_tool(app: &Rc<App>, state: &State, label: &str, action: Action) -> Page {
	let email = app.email;
	let password = app.password;
	let blocked = state.busy || !state.ready;
	button(app, label, "text-button", false, action)
		.reactive_attr("disabled", move || {
			if blocked || email.get().is_empty() || password.get().is_empty() {
				Some("true".into())
			} else {
				None
			}
		})
		.into_page()
}

fn account_settings(app: &Rc<App>, state: &State, account: &Account) -> Page {
	let mut children = vec![
		text("span", "eyebrow", "Account settings"),
		text("h1", "", "Sign-in methods"),
		text(
			"p",
			"intro",
			"Changes require fresh proof. Linking does not combine product permissions or transfer ownership.",
		),
	];
	if let Some(error) = &state.error {
		children.push(notice(error, true));
	}
	children.push(element(
		"div",
		"account-reference",
		vec![
			text("span", "", "Aidash Account"),
			text("code", "", &account.account_id),
		],
	));
	let count = account
		.identities
		.iter()
		.filter(|identity| identity.linked)
		.count();
	let identities = account
		.identities
		.iter()
		.map(|identity| {
			let mut heading = vec![text(
				"strong",
				"",
				if identity.linked {
					"Linked"
				} else {
					"Unlinked"
				},
			)];
			if identity.id == account.source_identity_id {
				heading.push(text("span", "badge", "Current session"));
			}
			let mut content = vec![
				element("div", "identity-heading", heading),
				element(
					"dl",
					"",
					vec![
						text("dt", "", "Issuer"),
						text("dd", "", &identity.issuer),
						text("dt", "", "Subject"),
						element("dd", "", vec![text("code", "", &identity.subject)]),
					],
				),
			];
			if identity.linked {
				content.push(
					button(
						app,
						"Unlink",
						"secondary compact",
						state.busy || count < 2,
						Action::AccountStart("/account/unlink/start", Some(identity.id.clone())),
					)
					.into_page(),
				);
				if count < 2 {
					content.push(text(
						"p",
						"small muted",
						"Keep at least one usable sign-in method.",
					));
				}
			} else {
				content.push(text(
					"p",
					"small muted",
					"The original Account association is retained. Add this method again using fresh proof.",
				));
			}
			element("article", "identity", content)
		})
		.collect();
	children.push(element("div", "identity-list", identities));
	children.push(
		button(
			app,
			"Add a sign-in method",
			"",
			state.busy,
			Action::AccountStart("/account/link/start", None),
		)
		.into_page(),
	);
	children.push(text("p","small muted","Recover lost access through your identity provider. This page cannot replace its recovery checks."));
	children.push(element(
		"section",
		"global-logout",
		vec![
			text("h2", "", "Sign out everywhere"),
			text(
				"p",
				"small muted",
				"End sign-in sessions across all products and devices. Work already admitted continues.",
			),
			button(
				app,
				"Sign out of all products and devices",
				"secondary",
				state.busy,
				Action::LogoutAll,
			)
			.into_page(),
		],
	));
	fragment(children)
}
