# Package indexes are built-in External services whose index responses the Egress gateway rewrites

Decided on 2026-10-19 while grilling Live previews. Execution Pods carry Node.js with npm and npx, and uv with uvx, so that agents can build and serve ordinary Web apps. The npm registry and PyPI become Built-in definitions of External services that the Deployment offers and each Tenant approves through Catalog Approval, as Sandboxed tools are (ADR 0034). npm reaches its registry through the Egress gateway with `replace-registry-host`, but PyPI's index names its files by absolute addresses on another host, which uv cannot redirect. For a package index, and only there, the Egress gateway therefore rewrites file addresses in index responses to its own paths, buffering those small responses up to a bound; package files still stream unchanged. `python_install` is removed, because uv through the gateway replaces it.

## Considered Options

- **Let the package indexes pass through a CONNECT tunnel.** No TLS would be intercepted, but methods could not be limited and paths could not be recorded, which weakens ADR 0054's guarantees. Rejected.
- **Run a pull-through package cache in the cluster.** It would solve the rewriting and cache packages, but adds a component to operate. Rejected for now.
- **Let each Tenant define its own package index External services.** Every Tenant would repeat the same definitions, and the rewriting must apply only to definitions Aidash controls. Rejected.
- **Keep `python_install` until Cloud has an Egress gateway (#310).** Two installation paths would coexist with different guarantees, and Cloud is not deployed yet. Rejected.

## Consequences

- uv never downloads a Python interpreter (`UV_PYTHON_DOWNLOADS=never`); the image's Python is the only one.
- Until #310 lands, Cloud and Dedicated Execution Pods cannot install packages.
