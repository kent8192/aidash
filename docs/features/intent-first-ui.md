# Precision Light and intent-first navigation

The user selected Design 1 on 2026-10-07: a white conversation surface,
cool-gray request history and one vermilion accent. The generated reference is
`01-precision-light-intent.png`. The conversation is the default entry point;
progress, files and agent administration are revealed where they are needed.

## Integration map

| Previous independent view | Existing owner                              | Preserved operations                                                                                              |
| ------------------------- | ------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| Working files             | Request → Files                             | Attachments, results, retention, recoverable cleanup, explicit deletion, restoration, references and capabilities |
| Generation                | Creator → Agent generation and approval     | Policies, requests, approvals and activation                                                                      |
| Authorization             | Trust → Access and accounts                 | Identity mappings, authority policies, simulations, credentials and revocation                                    |
| Semantic memory           | Settings → Registry → Sources and indexes   | Retrieval sources, indexing, provider configuration, failures and cleanup                                         |
| Transactions              | Request progress → Consistency and recovery | Subject-scoped transactions, operator trust, waiting, abort and recovery                                          |
| Node and connection       | Settings → General                          | Current identity, node, connected peers and disconnect                                                            |

Legacy paths and `settings?view=…` links resolve to these owners. Channel and
entity identities remain exact; an inaccessible request does not fall back to
another request. Management is reachable through the account menu. Graph View
is reachable from request progress and retains mesh and agent inspection.

## Design system

The existing React/Vite application uses the shadcn/ui New York components
([Vite installation](https://ui.shadcn.com/docs/installation/vite)) with Radix
primitives and Tailwind CSS. Components are vendored in `web/src/components/ui`;
`components.json` records the aliases and token stylesheet. Buttons, dialogs,
inputs, the composer, collapsibles, sheets and status badges share these parts.

Tokens: white surface `#FFFFFF`, background `#FAFAFA`, sidebar `#F0F1F2`, ink
`#18201F`, vermilion `#C74730`. The accent is slightly darker than the generated
reference to keep normal white button text readable. Dark mode uses the same
visual hierarchy with a lighter accent and dark text on primary actions.
Controls retain visible focus, localized accessible names, reduced-motion
support and responsive request history. The existing mesh-A logo geometry is
preserved; its symbol and wordmark are ink/white, its upper-right dash is vermilion, and its app tile is
light. Web and desktop icons are exported from that same SVG.

The Graph View visibility panel uses the same popover, border, text and icon
tokens. Enabled type and relation filters use the primary accent; disabled
filters use a neutral track and visible thumb. Its shadow and scrollbar also
follow the active theme.

## Event notifications

The existing event path is NATS → authorized server-sent events → query
refresh. The SSE broker subscribes to `aidash.<node>.events`; browser delivery
checks the active authority before emitting a frame. Broker events include
internal execution changes, so they are not displayed individually as toasts.

The dashboard uses [shadcn/ui Sonner](https://ui.shadcn.com/docs/components/radix/sonner)
to announce new approval/input requests and observed task transitions to
completed or failed. Notices are derived from the authorized state snapshots.
An initial snapshot, newly visible workspace or remote node seeds the baseline
quietly. Existing requests remain reachable in the notification center. Repeated
polling and stream reconnection do not reannounce the same pending request;
newly loaded terminal tasks are treated as history rather than a new transition.
Remote requests retain their node identity; remote task completion is not part
of the mesh snapshot and is not inferred from run status.

Updates arriving within 1.5 seconds are grouped. At most one toast is shown
every 30 seconds, with later updates collected into the next batch. A toast
stays for eight seconds. Clicking a single notice opens the exact request or
task and selects its authorized channel; clicking a batch opens the center,
where each item opens its corresponding resource. Opening the center clears
queued announcements. The center retains pending requests and up to 50 recent
important updates in this tab. Resolved or inaccessible resources are removed,
and changing the tab's authority or browser session resets notification state.

## Screenshots

These screenshots use synthetic API fixtures rather than production data.
The before view was rendered from base commit `6b886aac`; the after views
were rendered from implementation commit `28f54eed`.

- [Before: desktop workspace](intent-first-ui/before-light.png)
- [After: desktop conversation](intent-first-ui/after-light.png)
- [After: mobile conversation](intent-first-ui/after-mobile.png)
- [After: light Graph View filters](intent-first-ui/graph-filters-light.png)
- [After: dark Graph View filters](intent-first-ui/graph-filters-dark.png)

## Product requirements

The Notion v0.1 functional requirements were audited at implementation time:
[functional requirements](https://app.notion.com/p/3e172fa877aa8096bca5c8d8c2c73b24).
The six former views above were not named as independent screens. Their
capabilities remain required and are integrated, rather than deleted. Creator
and Trust are explicitly described in the
[product vision](https://app.notion.com/p/3d972fa877aa80ca8fd1f48b8d129d01).

This accepted intent-first design supersedes the older section 8 guidance
against a chat entry point. The remote Notion document has not been edited.
Server authorization, request/run controls, approval, and artifact provenance
remain authoritative. A count of completed tasks is not a goal-completion
verdict. Transaction and authorization recovery must remain usable during a
state visibility barrier. The error surface exposes Consistency and recovery
without relying on an ordinary state snapshot.

## Verification

- `npm run build` regenerates the Rust-owned OpenAPI client, type-checks and
  produces the Vite bundle.
- Collaboration unit tests: 48 passed, including 10 notification cases;
  graph unit tests: 69 passed before the notification follow-up.
- The UI suite covers 175 cases with API fixtures. Its full run passed 167; the
  eight cases with previous navigation/palette assumptions were updated and
  passed on recheck. The final palette and responsive surfaces were rechecked
  separately.
- Desktop browser transport/CSP tests: 8 passed. These are browser harness tests,
  not a packaged native-app launch.
- ESLint, Prettier and `git diff --check` passed; npm audit reported no
  vulnerabilities.

The Graph View palette follow-up was isolated in `fix/graph-filter-palette`,
with the uncommitted Design 1 baseline copied from `feat/intent-first-ui` and
verified byte-for-byte before editing. The source worktree remains intact.
TypeScript and the Vite production build passed. All 16 mesh-graph UI cases
passed; after correcting the legacy focus-ring color, the three filter and
responsive cases passed again. Light, dark, 390px and 900px screenshots were
visually inspected. The affected TSX and test files passed ESLint; the final
changes passed Prettier and `git diff --check`.

The notification follow-up passed TypeScript and the Vite production build.
All 13 focused UI cases passed, covering actionable toasts, grouping, throttling,
authority changes, exact remote-node identity, Japanese request creation,
existing notification navigation, and the intent/legacy/recovery surfaces.
Desktop light and 390px dark toast screenshots were visually inspected. The
golden-path browser launcher now uses the current Japanese New request entry.
ESLint, `trunk fmt`, `git diff --check`, and the production dependency audit
passed.

The backend acceptance suite requires its independent two-node environment and
was not run against the user's existing local backend.
