// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Declarative components shared by the Marketplace routes.
use super::super::hooks::{Load, Resource};
use crate::api::{ApiError, Plugin, PluginVersion, encode, price};
use reinhardt::pages::ui::{ActionButton, FormActionButton};
use reinhardt::pages::{Action, FormAction, FormRuntimeSource, IntoPage, Page, page};
use serde::{Serialize, de::DeserializeOwned};
use std::fmt::Display;
use std::rc::Rc;

pub fn link(href: impl Into<String>, class: &'static str, label: impl Into<String>) -> Page {
	let href = href.into();
	let label = label.into();
	// BFF navigation and protected downloads belong to the native service.
	let rel = if href.starts_with('/') {
		"external"
	} else {
		""
	};
	page!({
		a {
			href: href,
			class: class,
			rel: rel,
			{ label }
		}
	})
}

pub fn loading() -> Page {
	page!({
		div {
			class: "loading",
			role: "status",
			aria_label: "Loading",
			span {}
			span {}
			span {}
			span {
				class: "sr-only",
				"Loading…"
			}
		}
	})
}

pub fn empty(title: &str, content: Page) -> Page {
	let title = title.to_owned();
	page!({
		section {
			class: "empty",
			span {
				class: "eyebrow",
				"A place to begin"
			}
			h2 { { title } }
			p { { content } }
		}
	})
}

pub fn error_notice(error: ApiError, retry: Option<Rc<dyn Fn()>>) -> Page {
	let retry = retry.map(retry_button).unwrap_or(Page::Empty);
	page!({
		div {
			class: "notice error",
			role: "alert",
			div {
				strong { "Unable to complete this request" }
				p { { error.message } }
			}
			if error.status == Some(401) { { link("/auth/login", "button secondary", "Sign in again") } } else { { retry } }
		}
	})
}

fn retry_button(retry: Rc<dyn Fn()>) -> Page {
	page!({
		button {
			type: "button",
			class: "secondary",
			@click: move |_| retry(),
			"Try again"
		}
	})
}

pub fn form_error(message: Option<String>) -> Page {
	message
		.map(|message| {
			page!({
				div {
					class: "notice error",
					role: "alert",
					{ message }
				}
			})
		})
		.unwrap_or(Page::Empty)
}

/// Construct route-owned hooks outside the parent's dependency observer.
/// The resource state still controls replacement, while form field reads and
/// action initialization cannot make the parent remount a mounted form.
pub fn resource_view<T: Clone + Serialize + DeserializeOwned + 'static>(
	resource: Resource<T>,
	ready: impl Fn(T) -> Page + 'static,
) -> Page {
	let ready: Rc<dyn Fn(T) -> Page> = Rc::new(ready);
	page!(|resource: Resource<T>, ready: Rc<dyn Fn(T) -> Page>| {
		{
			match resource.get() {
				Load::Loading => self::loading(),
				Load::Error(error) => {
					self::error_notice(error, Some(Rc::new(move || resource.refetch())))
				}
				Load::Success(value) => reinhardt::pages::reactive::untracked(|| ready(value)),
			}
		}
	})(resource, ready)
}

pub fn form_submit<Form, Deps, T, E>(
	action: FormAction<Form, Deps, T, E>,
	idle: &'static str,
	active: &'static str,
) -> Page
where
	Form: FormRuntimeSource,
	Deps: Clone + PartialEq + 'static,
	T: Clone + 'static,
	E: Clone + Display + 'static,
{
	let label = page!({
		span { {
			if action.is_pending() { active } else { idle }
		} }
	});
	FormActionButton::new(action, label).into_page()
}

pub fn action_button<T: Clone + 'static, P: Clone + 'static>(
	action: Action<T, ApiError>,
	payload: P,
	class: &'static str,
	idle: &'static str,
	active: &'static str,
) -> Page {
	let label = page!({
		span { {
			if action.is_pending() { active } else { idle }
		} }
	});
	ActionButton::new(action, payload, label)
		.attr("class", class)
		.into_page()
}

pub fn permissions(values: &[String]) -> Page {
	let values = values.to_vec();
	page!({
		if values.is_empty() {
			p {
				class: "muted",
				"No permissions declared."
			}
		} else {
			ul {
				class: "permissions",
				for permission in values {
					li {
						code { { permission } }
					}
				}
			}
		}
	})
}

pub fn plugin_list(plugins: Vec<Plugin>) -> Page {
	page!({
		div {
			class: "plugin-list",
			for plugin in plugins { { plugin_row(plugin.clone()) } }
		}
	})
}

fn plugin_row(plugin: Plugin) -> Page {
	page!({
		article {
			class: "plugin-row",
			div {
				class: "plugin-glyph",
				aria_hidden: "true",
				{ plugin
				.name
				.chars()
				.take(2)
				.collect::<String>()
				.to_uppercase() }
			}
			div {
				class: "plugin-summary",
				span {
					class: "eyebrow",
					{ plugin.publisher_name }
				}
				h2 { { link(
					format!("#/plugins/{}", encode(&plugin.slug)),
					"",
					plugin.name.clone(),
				) } }
				p { { plugin.description } }
				div {
					class: "row-meta",
					span { { plugin.license } }
					{ plugin
					.latest_version
					.clone()
					.map(version_badge)
					.unwrap_or(Page::Empty) }
					span { { format!(
						"{} declared {}",
						plugin.declared_permissions.len(),
						if plugin.declared_permissions.len() == 1 {
							"permission"
						} else {
							"permissions"
						}
					) } }
				}
			}
			span {
				class: "price",
				{ price(&plugin) }
			}
		}
	})
}

pub fn versions(versions: Vec<PluginVersion>, plugin_id: &str, downloadable: bool) -> Page {
	let plugin_id = plugin_id.to_owned();
	page!({
		div {
			class: "version-list",
			for version in versions { { version_row(version.clone(), plugin_id.clone(), downloadable) } }
		}
	})
}

fn version_row(version: PluginVersion, plugin_id: String, downloadable: bool) -> Page {
	page!({
		article {
			class: if version.recalled {
				"version recalled"
			} else {
				"version"
			},
			div {
				class: "version-heading",
				h3 {
					code { { version.version } }
				}
				span {
					class: if version.recalled {
						"badge danger"
					} else {
						"badge"
					},
					{
						if version.recalled { "Recalled" } else { "Available" }
					}
				}
				span {
					class: "muted",
					{ format!("{} bytes", grouped(version.size_bytes)) }
				}
			}
			if version.recalled {
				p {
					class: "recall-reason",
					{ version
					.recall_reason
					.clone()
					.filter(|reason| !reason.is_empty())
					.unwrap_or_else(|| "This version must not be used for new Runs.".into()) }
				}
			}
			dl {
				dt { "SHA-256" }
				dd {
					code {
						class: "digest",
						{ version.digest }
					}
				}
				{ version
				.origin
				.clone()
				.map(version_origin)
				.unwrap_or(Page::Empty) }
			}
			if downloadable && !version.recalled { { link(
				format!(
					"/api/marketplace/plugins/{}/versions/{}/download",
					encode(&plugin_id),
					encode(&version.version)
				),
				"button secondary compact",
				"Download version",
			) } }
		}
	})
}

fn grouped(value: u64) -> String {
	let digits = value.to_string();
	let mut grouped = String::new();
	for (index, digit) in digits.chars().enumerate() {
		if index > 0 && (digits.len() - index).is_multiple_of(3) {
			grouped.push(',');
		}
		grouped.push(digit);
	}
	grouped
}

fn version_badge(version: String) -> Page {
	page!({
		code { { format!("v{version}") } }
	})
}
fn version_origin(origin: String) -> Page {
	page!({
		dt { "Origin" }
		dd {
			class: "break",
			{ origin }
		}
	})
}

pub fn publication_notice(version: PluginVersion) -> Page {
	page!({
		div {
			class: "notice success",
			role: "status",
			aria_label: "Publication result",
			{ format!("Version {} published. ", version.version) }
			code {
				class: "digest",
				{ version.digest }
			}
		}
	})
}
