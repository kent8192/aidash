// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Validated sign-in forms on the Pages form runtime.
//!
//! The generated form types are opaque, so each runtime is retained once and
//! exposed through type-erased handles. Passwords, e-mail addresses and codes
//! live only in these runtimes: they are never copied into a `Signal` of their
//! own, a URL or storage, and `clear` resets them.
use reinhardt::pages::{Page, form, page, use_form};
use std::rc::Rc;

/// Handle to the e-mail and password form.
#[derive(Clone)]
pub struct CredentialsForm {
	/// Renders the labelled fields; the flag disables them while busy.
	pub fields: Rc<dyn Fn(bool) -> Page>,
	/// Runs the form validators and reports whether the values are acceptable.
	pub validate: Rc<dyn Fn() -> bool>,
	/// Reactive: true once both fields hold a value.
	pub complete: Rc<dyn Fn() -> bool>,
	values: Rc<dyn Fn() -> (String, String)>,
	clear: Rc<dyn Fn()>,
	restore_email: Rc<dyn Fn(String)>,
}

impl CredentialsForm {
	/// Returns the current `(email, password)` pair as transient arguments.
	pub fn values(&self) -> (String, String) {
		(self.values)()
	}
	/// Resets both fields to empty.
	pub fn clear(&self) {
		(self.clear)()
	}
	/// Resets only the password field, keeping the address for another try.
	pub fn clear_password(&self) {
		let (email, _) = self.values();
		(self.clear)();
		(self.restore_email)(email);
	}
}

/// Handle to the verification-code form.
#[derive(Clone)]
pub struct CodeForm {
	/// Renders the labelled field.
	pub field: Rc<dyn Fn() -> Page>,
	/// Runs the form validators and reports whether the value is acceptable.
	pub validate: Rc<dyn Fn() -> bool>,
	value: Rc<dyn Fn() -> String>,
	clear: Rc<dyn Fn()>,
}

impl CodeForm {
	/// Returns the current code as a transient argument.
	pub fn value(&self) -> String {
		(self.value)()
	}
	/// Resets the field to empty.
	pub fn clear(&self) {
		(self.clear)()
	}
}

pub fn use_credentials_form() -> CredentialsForm {
	let definition = form! {
		name: CredentialsFormDefinition,
		fields: {
			email: EmailField { required }
			password: PasswordField { required }
		}
	};
	let runtime = reinhardt::pages::reactive::untracked(|| use_form(&definition).build());
	let (fields_runtime, fields_definition) = (runtime.clone(), definition.clone());
	let (validate_runtime, values_runtime, clear_runtime) =
		(runtime.clone(), runtime.clone(), runtime.clone());
	let (complete_runtime, restore_runtime) = (runtime.clone(), runtime.clone());
	let restore_definition = definition.clone();
	CredentialsForm {
		fields: Rc::new(move |busy| {
			let runtime = fields_runtime.clone();
			let definition = fields_definition.clone();
			page!({
				label {
					class: "field",
					"Email" input {
						type: "email",
						name: "email",
						autocomplete: "username",
						required: true,
						disabled: busy,
						bind: runtime.field(definition.email_field())
					}
				}
				label {
					class: "field",
					"Password" input {
						type: "password",
						name: "password",
						autocomplete: "current-password",
						required: true,
						disabled: busy,
						bind: runtime.field(definition.password_field())
					}
				}
			})
		}),
		validate: Rc::new(move || validate_runtime.trigger().is_ok()),
		complete: Rc::new(move || {
			let values = complete_runtime.watch().get();
			!values.email.is_empty() && !values.password.is_empty()
		}),
		values: Rc::new(move || {
			let values = values_runtime.get_values();
			(values.email, values.password)
		}),
		clear: Rc::new(move || clear_runtime.reset()),
		restore_email: Rc::new(move |email| {
			restore_runtime.set_value(restore_definition.email_field(), email)
		}),
	}
}

pub fn use_code_form() -> CodeForm {
	let definition = form! {
		name: CodeFormDefinition,
		fields: {
			code: CharField {
				required,
				max_length: 6
			}
		}
	};
	let runtime = reinhardt::pages::reactive::untracked(|| use_form(&definition).build());
	let (fields_runtime, fields_definition) = (runtime.clone(), definition.clone());
	let (validate_runtime, value_runtime, clear_runtime) =
		(runtime.clone(), runtime.clone(), runtime.clone());
	CodeForm {
		field: Rc::new(move || {
			let runtime = fields_runtime.clone();
			let definition = fields_definition.clone();
			page!({
				label {
					class: "field",
					"Verification code" input {
						name: "code",
						autocomplete: "one-time-code",
						pattern: "[0-9]{6}",
						maxlength: 6,
						required: true,
						bind: runtime.field(definition.code_field())
					}
				}
			})
		}),
		validate: Rc::new(move || validate_runtime.trigger().is_ok()),
		value: Rc::new(move || value_runtime.get_values().code),
		clear: Rc::new(move || clear_runtime.reset()),
	}
}
