# On Cloud, a Tenant Secret's hosts are sealed with its value

Decided on 2026-10-26 for #310. The host that mints Tenant Secret Capability Tokens can be compromised (ADR 0005), and if the broker trusted the token's destination alone, a compromised host could send every stored Tenant Secret to an address of its choosing. On Cloud and Dedicated each Secret Store version therefore holds the value together with the hosts it may be sent to, and the broker refuses any destination outside them. The application host can add versions but cannot read one, so it cannot attach new hosts to a value it does not know.

## Considered Options

- **Check hosts only where the token is minted**, against PostgreSQL. A compromised host could redirect stored values. Rejected.
- **Keep hosts as mutable metadata on the stored secret.** The application host can change that metadata, so it protects no more than the token. Rejected.

## Consequences

- On Cloud and Dedicated, changing a Tenant Secret's hosts requires entering its value again, which amends ADR 0051's separation between replacing a value and changing its hosts there. Self-hosted keeps hosts in PostgreSQL and changes them without the value.
- A compromised host can still use a stored value, but only toward hosts it was already allowed to reach, and only while the compromise lasts.
