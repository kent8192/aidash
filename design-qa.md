# Workspace layout design QA

Date: 2026-09-24

final result: passed

## Comparison targets and evidence

The first supplied light workspace is the primary layout target. The additional
images inform dark colors, attachments, channel details, and the adjacent thread
pattern; they are alternative examples rather than five simultaneous screens.

- Primary source: `/tmp/codex-remote-attachments/01a0d18a-ce9d-7183-856b-1e146e5b95cf/6664FD24-F58A-482C-8A69-BB424A841485/1-写真1.jpg`.
- Supplemental dark thread source: `/tmp/codex-remote-attachments/01a0d18a-ce9d-7183-856b-1e146e5b95cf/669F6A74-074F-4681-95F8-142B8899A692/3-写真3.jpg`.
- Brand source: the subsequently supplied `aidash-logo-kit.zip`. Its primary
  Mesh + Accent assets supersede the small logo visible in the screenshots.
- Rendered application: `http://127.0.0.1:18083/`, using the production frontend
  bundle and an in-memory API fixture. This preview does not connect to real
  workspaces, publish packages, or call an agent provider.

The following screenshots are produced by the reference-layout browser test in
`web/test-results/collaboration-workspace-re-c8ec2-ght-dark-and-thread-layouts/`:

| Capture                     | CSS viewport / image pixels   | State                                             |
| --------------------------- | ----------------------------- | ------------------------------------------------- |
| `workspace-light.png`       | 1280 × 800                    | Japanese, light, channel, approval pending        |
| `workspace-dark.png`        | 1280 × 800                    | Same data, dark                                   |
| `workspace-thread.png`      | 1280 × 800                    | Dark, Researcher thread open alongside channel    |
| `workspace-thread-tall.png` | 1280 × 960                    | Same thread state at the supplemental source size |
| `workspace-status.png`      | Element capture at 1280 × 800 | Right panel including its action button           |
| `workspace-composer.png`    | Element capture at 1280 × 800 | Composer and formatting/attachment actions        |

Source pixels are 1280 × 800 for the primary image and 1280 × 960 for the thread
image. Implementation captures use device scale factor 1 and exclude browser
chrome. No density resampling was needed. Source and implementation were opened
together in the same comparison input. Focused status/composer captures were
also opened alongside the primary source to inspect labels, spacing, and controls.

The fixture reproduces the release channel's content roles, not every fictional
record in the examples. Task counts, agent versions, requests, and dependency edges
come from API-shaped records. The supplemental source contains existing replies;
the captured implementation thread has just been opened, so it correctly contains
its root and an empty reply composer. This content difference is not a typography
or spacing comparison.

PR review captures are committed in `docs/images/workspace-layout/`: `before.png`
shows the previous interface at base commit `10fb917`, `workspace-light.png` shows
the updated interface with the same fixture and viewport, and
`workspace-thread-tall.png` shows the dark thread layout. The baseline was built
from an isolated archive of the base frontend using the same mock data.

## Findings and iteration history

No actionable P0/P1/P2 layout findings remain within the implemented scope.

1. **Resolved P2 — excessive header and message density.** Earlier captures hid
   the first message and wrapped the sidebar thread entry. Compact header/goal
   spacing and the short visible thread label restore the reference's hierarchy.
   The final light capture shows the first author, full text, and composer.
2. **Resolved P2 — miniature graph became illegible.** Automatic graph fitting
   shrank nodes and labels. The miniature now uses a fixed readable scale,
   recentering on resize, and theme-aware label colors. The focused status capture
   shows actual task-dependency edges and readable agent labels.
3. **Resolved P2 — status footer and date separator were partially clipped.**
   Reduced section/task spacing and a sticky date separator fix the initial
   1280 × 800 view. Final browser measurements report status scroll height equal
   to its client height (646 px) for this fixture. Longer real histories retain
   scrolling; persistent composer and execution controls remain outside it.
4. **Brand follow-up completed.** Replaced provisional marks with the supplied
   wordmark and application icon; also applied the light wordmark to login and
   the supplied favicon. All four installed assets were compared byte-for-byte
   with the archive. The browser successfully loaded both workspace SVG assets.

## Required fidelity surfaces

- **Typography:** the existing DM Sans / Noto Sans JP stack supplies Latin and
  Japanese text. The reference's exact font metadata is unavailable; the visible
  sans-serif hierarchy, compact weight, message leading, and subordinate metadata
  are retained. The supplied wordmark has outlined lettering and uses no font
  recreation. Long dynamic names and goals can wrap.
- **Spacing and layout:** 42 px header, 48 px rail, 184 px channel sidebar, flexible
  conversation, and 262 px status panel follow the primary frame. The right pane
  widens to 330 px for a thread. Borders, small corner radii, inset goal banner,
  approval card, and bottom composer were compared in the final captures.
- **Colors:** forest navigation, pale sage selection and status surfaces, muted
  metadata, and coral approval actions follow the primary reference. Dark mode
  uses deep green surfaces, light text, and separate semantic warning/error
  colors. State does not depend on color alone.
- **Images and icons:** original brand SVGs retain proportions and colors. Standard
  UI actions use the existing icon library; agent avatars are role icons. The
  examples' fictional human photographs are not inserted as actual user profiles.
  There are no generated or reconstructed brand assets.
- **Copy and content:** Japanese/English labels describe actual actions. Task
  assignment does not claim membership invitation, name insertion does not claim
  notification delivery, and channel pause controls observed runs rather than a
  persisted goal-wide lifecycle. Graph counts do not claim unobserved federation
  health. Existing settings and execution details remain reachable.

## Interaction and regression verification

- Build, scoped ESLint, formatting, and `git diff --check`: passed.
- Collaboration/unit projections: 35 passed; agent graph units: 19 passed.
- Final collaboration browser suite: 32 passed after the layout and logo changes
  and integration of the base branch's graph focus fix.
  It covers message/thread persistence and isolation, retry identities, stale
  authorization failures, attachment validation/retry/download/preview, approval
  decisions, channel run control, search, notifications, theme persistence, IME,
  and escaped message formatting.
- The full isolated UI suite passed 69 tests on the final implementation after
  integrating `develop/0.1.0` at `10fb917`.
- Narrow layouts at 390 × 844 and 900 × 400 keep the composer, send action, and
  status action in the viewport without document horizontal overflow.
- The local in-app browser additionally verified the file preview, theme switch,
  adjacent thread, and successful brand loading. No console errors appeared
  during the final reload and interaction pass. Browser viewport overrides were
  reset before handoff.

The isolated browser suite mocks the API boundary. Real PostgreSQL/NATS/worker,
OIDC-provider, and remote federation integration were not exercised by this task.
The existing ExcelJS bundle-size warning remains. This is not a full accessibility
audit or a remote CI result.

## Follow-up polish and product boundaries

- P3: the screenshot font and photographic profile assets are not supplied; current
  project fonts and actual available identity data remain authoritative.
- P3: names include versions to preserve existing registry disambiguation, making
  some labels longer than the fictional source labels.
- Membership invitations, huddles, reaction persistence, and broader goal/admission
  architecture are outside the implemented layout workflow; the UI does not expose
  nonfunctional controls for them. Current server-contract boundaries are documented
  in `docs/collaboration-ui.md`.

## Implementation checklist

- [x] Dedicated task branch and worktree; original checkout unchanged.
- [x] Reference layout, dark theme, adjacent threads, and responsive navigation.
- [x] Functional composer attachments, previews, approval, and observed-run controls.
- [x] Supplied original brand assets installed and visually checked.
- [x] Final captures compared, required fidelity surfaces reviewed, and checks passed.
- [x] Local sample preview retained for inspection.
