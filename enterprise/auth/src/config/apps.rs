// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Installed app registry for auth.
//!
//! `reinhardt-admin startapp` automatically appends new apps here.

use reinhardt::installed_apps;

installed_apps! {
	// Apps will be added here by `reinhardt-admin startapp`.
	authentication: "authentication",
}
