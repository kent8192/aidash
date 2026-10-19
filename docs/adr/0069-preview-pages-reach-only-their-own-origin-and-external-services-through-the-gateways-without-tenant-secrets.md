# Preview pages reach only their own origin, and External services only through the gateways without Tenant Secrets

Decided on 2026-10-19 while grilling Live previews. Preview pages run in the viewer's browser, outside the Execution Pod and the Egress gateway, so a page that could fetch any address would send Working Area content out without a Web disclosure approval or an External service (ADR 0049, ADR 0054). Every preview response therefore carries a Content Security Policy that confines the page to its own origin. A Live preview page may reach the External services bound to its Run only through a path on its own origin that the Preview gateway forwards to the Egress gateway. Those requests are limited to the declared methods and recorded like Sandboxed code's, but never carry a Tenant Secret, because a viewer may have no authority to use it. Code that needs the secret runs in the Live preview's Web server inside the Pod. File previews reach nothing but their own origin.

## Considered Options

- **No restriction in the browser.** Working Area content could leave through the viewer's browser unseen. Rejected.
- **A Content Security Policy that admits the External services' origins directly.** Requests would go unrecorded, methods unenforced, and the viewer's own cookies and address would reach the service. Rejected.
- **The gateway path attaching Tenant Secrets.** Any viewer could spend the secret from the browser's developer tools. Rejected.

## Consequences

- Scripts, styles and fonts loaded from public CDNs do not work in previews. An app bundles its dependencies or reaches them through a bound External service.
