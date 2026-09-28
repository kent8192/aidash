# Creator Workbench visual specification

The Overview follows `overview-reference.jpg`. The user approved the four-tab
ImageGen board in `approved-tabs.png` on 2026-09-28.

- Overview: compact profile and configuration cards, conversation sandbox, and
  a factual validation/dependency/version rail.
- Build: a wider configuration editor with a summary rail.
- Test: environment controls, conversation, tool activity, usage, and history.
- Versions: registered versions, saved-draft comparison, and version provenance.
- Register: version and release notes, a draft review, and technical validation.

All product content comes from the existing Registry and Workbench APIs. Empty
states remain empty. No reference-image agent, conversation, person, model,
connection status, timestamp, or certification is seeded into the application.
The existing icon library and Aidash assets are reused. User profile icons remain
user data. The global application navigation is preserved.

Registration is Registry admission, not Marketplace publication or external
certification. The registration action requires a successful technical validation
of the current saved revision. Editing invalidates the displayed validation.
The sandbox defaults to a real isolated connection; an unconfigured connection
cannot send. The existing explicit simulated-tool mode remains available and is
clearly labeled. Real mode submits no simulated tool fixtures.

Validation commands:

```sh
npm run build --prefix web
cd web
npx eslint src/workbench.tsx tests/workbench.spec.ts
AIDASH_E2E_URL=http://127.0.0.1:18477 npm exec playwright test tests/workbench.spec.ts
```

The browser regression suite uses test-only intercepted responses, as before;
these are not shipped or used by the application. This suite is separate from
live-data visual acceptance.
