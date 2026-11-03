// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Full-stack auth app generated with startapp --with-pages.
#[cfg(client)]
pub mod client;
#[cfg(server)]
pub(crate) mod models;
pub mod serializers;
#[cfg(server)]
pub mod server;
pub mod server_fn;
pub mod services;
pub mod urls;
#[cfg(server)]
#[reinhardt::app_config(name = "authentication", label = "authentication")]
pub struct AuthenticationConfig;
