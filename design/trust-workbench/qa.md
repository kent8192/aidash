# Trust Workbench design QA

## Outcome

final result: blocked

Implementation and automated regression checks are complete. The remaining gate is an authenticated run against the user's real local node. The in-app browser at http://127.0.0.1:5174/trust still shows the Google sign-in screen. No mock data was added to the application and no authentication bypass was introduced. Browser regression tests use isolated pre-existing test fixtures; those captures are not evidence of successful live-data integration.

## Visual sources and evidence

- Overview source: `/tmp/codex-remote-attachments/01a0e7bf-e903-7e90-9a1d-773bd96d626a/CDDE19C2-38E8-4D8C-BAED-91FBB4B1BF79/1-写真1.jpg` (1280 × 960).
- Approved detail sources: `design/trust-workbench/{policies,audit,certifications,incidents}.png` (1448 × 1086, four designs approved in this conversation).
- Desktop implementation captures: `web/test-results/workbench-Trust-detail-tab-35fc2--incident-filtering-at-1280/{overview,overview-details,policies,audit,certifications,incidents}.png`.
- Mobile implementation captures: `web/test-results/workbench-Trust-detail-tab-df435-d-incident-filtering-at-390/` with the same names.
- CSS viewports: desktop 1280 × 960; mobile 390 × 960; device scale factor 1. Detail source images are compared at 1280/1448 scale. The existing app scrolls inside its content area, so captures show current scroll positions rather than the entire scrollable document. `overview-details.png` covers the lower cards; mobile incident capture covers the focused form.
- States: Overview with no permission context; Policies before checking; Audit with a selected event; Certifications unavailable; Incidents filtered to open records and incomplete optional evidence. Nonempty test records are fixture data isolated to the test runner.

## Comparison and fixes

1. P2: Global `dl` grid styling compressed observation fields into narrow columns. Changed `.trust-facts` to a scoped block layout. Subsequent desktop captures show readable source, version and timestamps.
2. P2: Workbench stylesheet load order overrode the three-column permission form and compact Overview typography. Loaded the Trust stylesheet after the base Workbench stylesheet and scoped the context grid. Subsequent captures show three input columns on desktop and stacked fields on mobile.
3. P2: Existing content padding duplicated the Trust canvas gutter. Scoped the content container padding/background to Trust screens.
4. P2: The initial unassessed matrix lacked the approved table headings. Kept column headings visible before evaluation, with an explanatory empty state inside the table. Global light table row styling then appeared; scoped row/cell styles corrected it in the final captures.
5. Functional regression: The new incident test used an exact label locator that included select option text. Changed the test to target the combobox's accessible name. Both viewport cases pass.

## Fidelity surfaces

- Typography: existing application sans-serif and icon library retained; compact 11–14px data labels and 24px identity/assessment hierarchy. Long IDs wrap rather than overflow. Source images use similar compact sans-serif typography.
- Layout: Overview uses five metric cards, a three-column content grid and a 280px summary rail; details use the approved wide-content/narrow-rail structure. Policies has summary tiles, context form and matrix; Audit has timeline plus event inspector; Certifications has a clear unavailable state plus three evidence navigation cards; Incidents has list and report form columns. All collapse at smaller widths.
- Color: white cards on a sage canvas, forest-green navigation and icons, and pale semantic status badges, following the shared Creator/Trust design merged in #69. The original approved dark proposals remain layout references. No fabricated green trust verdict.
- Assets: existing Aidash brand and application navigation retained. Lucide icons replace the generated design icons; no fabricated user portrait, certification marks or agent logo was added.
- Copy/content: UI uses authenticated inspection, contextual permission, incident and audit endpoints. Unavailable assessments/certifications are explicitly unavailable. Generated copy implying submission to an unsupported assessor or all-tenant access was not implemented. Original screenshot numbers, users, certifications and issue descriptions are not production data.

Focused visual checks covered observation fields, policy input alignment/table colors, metric typography, incident evidence validation and mobile form boundaries. Full-view comparisons covered each main region. Remaining visual differences in the shared application shell are intentional: current Aidash branding, global navigation and locale controls remain intact.

## Verification

- Server-owned OpenAPI generation and production frontend build completed successfully.
- TypeScript project check, ESLint on changed TypeScript files and Prettier checks passed.
- Workbench regression suite: 24 tests passed, including Creator compatibility and Trust desktop/mobile behavior.
- After integrating #69, the suite also checks that registered capabilities remain visible before permission evaluation at both viewport widths.
- Tested navigation from evidence links, contextual permission result labels, audit event selection, pagination/reset, incident status/search filters, required notes, paired optional evidence validation, horizontal overflow and JavaScript errors.
- No captured JavaScript errors in the test cases. Existing Vite large-chunk warning remains.
- Live authenticated API rendering, report downloads and actual incident writes were not performed. No incident was created on the user's node for a visual test.

## Remaining step

Sign in to the local application, select an existing registered version, and verify all five tabs and permission inspection against its visible real records. Then capture the authenticated views and update this report. Do not treat the fixture captures as completion of this gate.
