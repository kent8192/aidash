# Lifting a suspension may resume the work it paused

Decided on 2026-11-12 in #876. Supersedes ADR 0086's rule that lifting a common suspension never resumes work automatically. Fresh sign-in after a restore is still required.

A Run paused because its Account was suspended records the reason `account_suspended` and the suspension epoch it was paused under. An Authentication Administrator who restores the Account may choose `resume_paused_work`, which is off by default. Their choice stands in for each owner's manual resume, and only for Runs paused by that suspension. The product finds these Runs on its periodic validity pass. It rechecks each Run's original External Identity, Mapping, credential and local permissions before resuming it. This is the only path that rebinds a Run's suspension epoch without its owner. A Run that fails any recheck stays paused.

An owner may also resume, in one dashboard action, every Run of theirs paused by a suspension. Each Run gets the same checks as a manual resume.

Recovery of a person who lost every sign-in method stays with GCIP or the upstream identity provider. On Deployments with common sign-in, the dashboard asks a person with only one usable sign-in method to add another, and links to the Authentication Server's account page. Dismissing the request hides it for 30 days.

## Considered Options

- **Keep manual resume only.** Rejected: an owner has to find and resume each Run of a lifted suspension, and unattended Runs sit until someone notices.
- **Resume automatically on every restore.** Rejected: a suspension is a deliberate stop, so a person decides whether its work goes on.
- **Leave it to the shared outage-resume behavior (#875).** Rejected: a suspension is not an outage.
- **Administrator-assisted rebinding or recovery codes for lost sign-in.** Deferred: either needs its own proof and ownership-recovery contract. Prompting for a second method prevents the common case of a person with only one enterprise sign-in.

## Consequences

- Restoring with `resume_paused_work` resumes only work paused by that suspension epoch. Work paused for another reason, or by an earlier suspension, is untouched.
- The Authentication Server records the resume choice per Account and epoch. The product applies it, because the Authentication Server cannot push to products.
