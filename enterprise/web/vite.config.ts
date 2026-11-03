// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { dashboardConfig } from "../../web/vite.config.ts";

// The Enterprise build is the dashboard with these sources in its slots.
export default dashboardConfig(dirname(fileURLToPath(import.meta.url)));
