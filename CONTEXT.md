# Aidash

Aidash runs and governs AI agents for tenants across federated nodes. This glossary fixes the terms used in issues, code and documentation.

## Language

### Tenancy and authorization

**Tenant**:
An isolation boundary that owns subjects, policies, catalog approvals and Provider Credentials.
_Avoid_: Aidash tenant, organization, customer

**Tenant-owned**:
Said of data that exists for exactly one Tenant, such as its Registry, Workspaces, Runs, memory and Provider Credentials. Work done for one Tenant never reads or changes another Tenant's Tenant-owned data.
_Avoid_: tenant-scoped (unqualified), tenant data

**Deployment-wide**:
Said of data that belongs to the Deployment and to no Tenant, such as External Identities, Deployment Administrator grants and Registration Requests.
_Avoid_: global, shared, common

**Deployment Scope**:
Work that reaches the Tenant-owned data of every Tenant at once, for one declared reason, such as finding due Runs or purging expired evidence. Other work either happens within exactly one Tenant, including a Deployment Administrator's work on a single Tenant, or touches only Deployment-wide data, such as signing in, and reaches no Tenant's data.
_Avoid_: bypass, superuser access, cross-tenant access, admin mode

**GCIP Tenant**:
An isolated user pool in Google Cloud Identity Platform with its own sign-in methods and identity providers. It is never a Tenant by itself. GCIP is part of Enterprise, so only Cloud and Dedicated Deployments have GCIP Tenants and the concepts built on them: Shared Pool, Tenant Binding, Sign-in Domain and MFA Requirement.
_Avoid_: Tenant (unqualified), identity tenant, realm

**Shared Pool**:
A GCIP Tenant with no Tenant Binding. Its External Identities may hold a Personal Tenant, create Tenants and join any Tenant through Invitations. A Deployment names at most one; a GCIP Tenant that is neither its Shared Pool nor bound admits nobody, and an External Identity whose GCIP Tenant changes between shared and bound is disabled.
_Avoid_: public pool, default tenant, open tenant

**Tenant Binding**:
The optional one-to-one association between a GCIP Tenant and the only Tenant its users may be mapped into. Only a Tenant that brings its own single sign-on has one. Its users never hold a Personal Tenant, create Tenants or join another Tenant through an Invitation, whatever the Tenancy Rules say.
_Avoid_: tenant mapping (collides with Mapping)

**Tenant name**:
The identifier of a Tenant, in the form `<adjective>-<noun>-<random string>`, such as `brave-otter-k3m9q2xa`. Aidash assigns it to every Tenant, including those a Deployment Administrator creates, and nobody chooses it; it reveals nothing about the Tenant or its people, it never changes, and it is never reused, even after its Tenant is purged. People never type it to sign in.
_Avoid_: organization identifier, org, org ID

**Tenant label**:
The human-readable name of a Tenant, chosen and changed freely by those who manage it. It never identifies or routes to a Tenant.
_Avoid_: Tenant name, display name (collides with Display Attributes)

**Personal Tenant**:
A Tenant that Aidash creates for one External Identity at sign-in, where the Tenancy Rules allow it. That External Identity is its Tenant Creator. It is otherwise an ordinary Tenant: its administrators may invite others into it. An External Identity has at most one at a time; Aidash never re-creates one at sign-in, and once it is deleted, creates another only when the person asks for it, first offering to restore the deleted one. A deleted Personal Tenant restored after its replacement was created is no longer that identity's Personal Tenant, yet still counts toward no Tenant-creation limit.
_Avoid_: private tenant, user tenant, home tenant

**Tenancy Rules**:
The deployment-wide rules for how Tenants come to exist and how people join and leave them: whether Personal Tenants are created, who may create Tenants and how many, how many Nodes a Tenant may have, and whether Invitations exist. Each Offering supplies their defaults and the Deployment Administrators may change them; no Tenant's policy decides them.
_Avoid_: tenancy policy (collides with a Tenant's policy), tenant settings

**Tenant Deletion**:
A recorded decision to delete a Tenant. From that moment nobody can use the Tenant, and its running and queued work is cancelled rather than paused. Until the grace period ends its Tenant Administrators or a Deployment Administrator can restore it (only a Deployment Administrator, when one deleted it), which brings back its data and configuration but not cancelled work, revoked Plan Connections or revoked Invitations; afterwards its data is purged, while its audit history is kept for its own retention period.
_Avoid_: soft delete, deactivation, archive

**Leaving a Tenant**:
A member's own decision to end their membership by disabling their own Mapping. Aidash refuses it when it would leave the Tenant without a Tenant Administrator, and a member mapped through a Tenant Binding cannot leave. A member who left returns only through a new Invitation, as the same user subject.
_Avoid_: unsubscribe, quit, self-removal

**Tenant Creator**:
The External Identity that created a Tenant through Aidash. It receives that Tenant's first Mapping and becomes its first Tenant Administrator. It is a historical fact, not a standing role: another Tenant Administrator may later remove its administration. A Tenant a Deployment Administrator creates has no Tenant Creator; its first administrator joins through an Invitation. Being a Tenant Creator never confers Deployment Administrator authority.
_Avoid_: owner (the Tenant Creator's subject name), founder

**Sign-in Domain**:
A verified email domain that routes sign-in to exactly one Tenant Binding's GCIP Tenant; addresses in no Sign-in Domain sign in through the Shared Pool. It only routes; it never grants or decides authority.
_Avoid_: domain (unqualified), home realm, domain route, authorized domain

**Catalog Approval**:
A decision, by a subject its Tenant's policy permits, that one exact version of a definition in the Tenant's Registry may be used on the Nodes of its Approval Scope, optionally only by certain subjects or Assignable Groups there. The restriction applies to whoever uses the definition directly: for an agent version, who may start its Runs; for a definition an agent depends on, which agents may use it. Built-in definitions need none, except Sandboxed ones, Page reading and Provider tools. Approving an agent version also makes that agent a subject of the Tenant's policy, so that it can run. It can be withdrawn but never turned into a standing refusal. It governs use and what is offered for use: withdrawing it stops the Runs using that version and takes the version off offer, and who may read what Runs have already produced still depends on `registry.read`.
_Avoid_: installation (taking something from the Marketplace into the Registry), operator approval, deny

**Approval Review**:
What one approver sees and decides for one definition and one Node at once: every definition it needs, followed transitively and including any the Node binds to it by default, each with what it enables, its exact version and what still keeps it from being usable. The approver approves, in one decision, any part of it they are permitted to approve, and nothing outside what they reviewed. A definition is ready only once the whole of its Approval Review is approved.
_Avoid_: approval set (the Marketplace operation), install plan, dependency approval

**Catalog Approval Request**:
A person's request, to whoever may approve, that one definition and everything its Approval Review names be approved on one Node, with a reason. It grants the requester nothing; an approver approves through an Approval Review or declines it with a reason, and it no longer counts once what it names changes.
_Avoid_: approval ticket, access request, Registration Request

**Approval Scope**:
The Nodes a Catalog Approval covers, chosen by its approver: one Node, or every Node of the Tenant on the same Deployment, including Nodes created later. A Node on another Deployment is never in another Node's Approval Scope.
_Avoid_: tenant-wide approval, global approval

**Version Carry-over**:
An approver's choice, made with a Catalog Approval, that later versions of the same definition are approved too, as long as what the approval consented to is unchanged: what the version may use, where it sends content and how long it is kept there. A later version that changes any of these is not carried and waits for an Approval Review.
_Avoid_: auto-approval, approval inheritance, preapproval

**Carried Approval**:
A Catalog Approval that Version Carry-over gave a later version, recording the approval it came from. Once given it is an ordinary Catalog Approval: it can be withdrawn on its own, and withdrawing the approval it came from leaves it in place.
_Avoid_: inherited approval, automatic approval

**Subject Credential**:
A revocable bearer token issued by the policy service to a subject in a Tenant. A member may issue one only for its own subject, and it ends when the Mapping it was issued through is disabled; one issued for a service or agent subject belongs to the Tenant.
_Avoid_: Credential (unqualified), API key, personal access token

**Child Credential**:
A Subject Credential issued by presenting another Subject Credential, its parent, for the same subject. It expires no later than its parent, ends when its parent ends, and cannot issue a Child Credential of its own.
_Avoid_: derived credential, sub-token, scoped token

**Host Access**:
Reaching a Deployment through its own settings and database on the machine that runs it, outside every HTTP route and every policy check. It is a way in, not a role: whoever has it can issue the first Subject Credential of a Tenant before any Mapping exists, which no Deployment Administrator can do over HTTP. Records it makes name it as their issuer unless told otherwise.
_Avoid_: host operator, Operator, Host Administrator, root

**Node Registration**:
A Tenant Administrator's decision that one of their Tenant's Nodes accepts Federation from one named Node of another Tenant, trusting that Node's Deployment Key and the Key Rollovers it signs. Within one Tenant no Node Registration is needed, because the Tenant's policy decides.
_Avoid_: peer, peering, Peer Credential (removed), trust (unqualified)

**Peer Mapping**:
A receiving Node's mapping of one subject of a registered Node in another Tenant to a subject of its own Tenant, under whose authority that subject's requests are decided.
_Avoid_: federation mapping, remote user, Mapping (unqualified, an External Identity's link)

**Authorization Decision**:
The recorded outcome of one policy check: who asked to do what to which resource, under which policy revision, and why it was allowed or denied. Together they are part of a Tenant's audit history. They are not an access log: because of Read Coalescing, the absence of a row does not mean a read was not checked.
_Avoid_: access log, audit row, Decision Evidence (an Approval Judge record), decision (unqualified)

**Read Recheck**:
The check, repeated at each worker boundary and every 30 seconds for a running command or Live preview, that everyone on the Run's delegation chain may still read everything the Run has read and everything the Runs it read from have read. It hands over nothing new; it is what makes a revoked permission to read stop a Run. It never looks at a Catalog Approval; that is the Use Recheck's job. No outcome is remembered from one Read Recheck to the next. A Node that does not answer has not refused: a Run pauses at its worker boundary, a running command goes on to its own limit, and a Live preview stops after five misses in a row.
_Avoid_: read walk, provenance walk, revalidation

**Continuation Check**:
The light check, on each Egress request and each pass over a running command, that a Run is not stopped and may still run. It never looks at what the Run has read; that is the Read Recheck's job.
_Avoid_: liveness check, Run gate

**Use Recheck**:
The check, at each worker boundary, on each request to resume a Run and every 30 seconds for a running command or Live preview, that the Run may still use every definition its Binding names under their current Catalog Approvals. It is what makes a withdrawal stop the Runs using that version. It never looks at what the Run has read.
_Avoid_: guard, approval recheck

**Read Coalescing**:
The rule that repeating a check with the same outcome adds no new Authorization Decision: one for each Run, subject, action, resource, policy revision and outcome, naming the Run the check was made for. It covers the Read Recheck, the Use Recheck, the Continuation Check, a Run's own reads, and the Read dependencies checked when a person opens a Run's output. What a person opens is recorded on every request. Checks that make something happen, and denials that refuse a request or stop a Run, are recorded every time.
_Avoid_: audit dedupe, audit sampling, deduplication

**Read dependency**:
Something a Run read, recorded with the Run: a Task, Message, memory record or definition, or an Agent it found on another Node. A Run's outputs are shown only to a reader who may still read every Read dependency of that Run and of the Runs it read from, and one held by another Node is confirmed with that Node, except an Agent found on another Node of the same Tenant, which that Tenant's policy decides without asking. When that Node does not answer, the output stays hidden and the reader is told the Node could not be reached; that is not a refusal. The exception is the Run that produced the output: it always sees its own outputs, because its own Read Recheck is what checks its Read dependencies.
_Avoid_: provenance, taint, dependency (unqualified)

**Unconfirmed item**:
An item left out of a list, search or tool result because a Node holding one of its Read dependencies did not answer. The response says how many there are for each Node; it is not a refusal, and the item can still turn out to be refused once that Node answers.
_Avoid_: hidden item, omitted item, missing item

### Bring your own key (BYOK)

**Provider Credential**:
A Tenant-owned record of one model-provider API key, bound to a single Provider Catalog entry. It holds only non-secret metadata and a reference to the stored Key Material.
_Avoid_: Credential (unqualified), BYOK credential, provider key

**Key Material**:
The plaintext provider API key itself. It is accepted once and is never returned, persisted outside the Secret Store, or logged.
_Avoid_: Secret, token

**Secret Store**:
The store that holds Key Material, Plan Tokens and the values of Tenant Secrets. On Cloud and Dedicated the application host can only write to it; on a self-hosted node the server can also read it. When none is configured, Provider Credentials, Plan Connections and Tenant Secrets are unavailable.
_Avoid_: Provider Credential Store (former name), Vault, Secret Vault, secret backend, self-hosted store

**Master Key**:
The key, supplied by whoever runs a self-hosted node, that protects its Secret Store. It lives outside the database and outside Registry configuration; a node without one has no Store.
_Avoid_: Encryption key, store key, KEK, Key Material

**Provider Catalog**:
The closed, code-defined list of model providers Aidash calls with a grant, each with a fixed base URL and the one kind of grant that serves it: a Provider Credential or a Plan Connection. A Keyless endpoint is not in it.
_Avoid_: Provider allowlist

**Keyless endpoint**:
The endpoint of a model or embedding definition that Aidash calls without any grant. It lies outside the Provider Catalog, and Aidash promises nothing about what it retains. A Tenant's Keyless endpoint must be publicly reachable unless the Deployment Administrators of the Deployment making the call permit its private origin. No Data Protection Level governs it; a Tenant may forbid Keyless endpoints, and then no Run whose delegation chain carries that Tenant's content calls one, nor does a chain that includes a Tenant on another Deployment. A Deployment-hosted model is never forbidden this way, since nothing sent to it leaves the Deployment.
_Avoid_: unauthenticated provider, local provider, open endpoint, self-hosted model

**Data Protection Level**:
A setting of a Tenant's policy for what Aidash asks of OpenRouter whenever it calls a model there with the Tenant's content: No Restriction asks nothing, No Training sends the content only to endpoints that never train on it, Safety Retention only to endpoints that keep it at most to prevent abuse, and Zero Data Retention only to endpoints that do not keep it. Aidash refuses, rather than falls back from, a call that would not meet it. It governs nothing else Aidash sends: Research, Keyless endpoints, Plan Connections and Tenant tools are outside it. New Tenants start at No Training. A Deployment may set a floor its Tenants cannot go below, and a delegation chain is held to the strictest level of any Tenant in it, a Tenant on another Deployment counting as Zero Data Retention. Wherever Aidash requires zero data retention it is strict: an endpoint with only Safety Retention never meets it. OpenRouter screens all traffic, zero data retention included, for child sexual abuse material and may keep flagged content for review and legal preservation; no level avoids that exception.
_Avoid_: ZDR mode, privacy mode, Zero Data Retention requirement (former name), Standard (former name of No Restriction), model-call ZDR, model-call level

**Safety Retention**:
An endpoint's published promise that content is never trained on but may be kept, from any request, only to detect and prevent abuse. It sits between no training and zero data retention and never meets zero data retention.
_Avoid_: ZDR with safety retention, ZDR compatible, soft ZDR

**Provider Route**:
The one OpenRouter endpoint a model version pins every request to, named by a leaf endpoint tag: a tag no other endpoint of that model sits under. Its approval is time-bounded, and the endpoint must be on OpenRouter's zero data retention list whenever a request goes out. When it is unusable, the request is refused with every reason that applies, never sent elsewhere.
_Avoid_: route pin, endpoint pin, provider tag, pinned route

**Media Route**:
An OpenRouter endpoint approved, for a bounded time, to receive the named image or audio formats of one model version, named by a leaf endpoint tag like a Provider Route. A model version with a Provider Route sends media only through a Media Route for that same tag; one without may use any of its usable Media Routes.
_Avoid_: media endpoint, media pin

**Provider Credential Binding**:
A Tenant's choice of which Provider Credential serves a given Provider Catalog entry. A Registry configuration declares that it needs a tenant key; the binding supplies it.
_Avoid_: Credential reference, key assignment

**Fingerprint**:
A short, Tenant-keyed digest that identifies Key Material within one Tenant without enabling cross-Tenant correlation.

**Revocation**:
The irreversible end of a Provider Credential's use. A revoked Provider Credential can only be deleted.
_Avoid_: Disable, suspend

**Provider Authorization**:
The provider-hosted consent through which a Tenant obtains a Provider Credential. It is the only way to obtain one; Key Material is never typed or pasted into Aidash.
_Avoid_: Key upload, key entry, connect flow

**Plan Connection**:
One subject's grant, obtained through Sign in with ChatGPT, that lets Runs whose delegation chain starts at that subject draw on that person's ChatGPT plan allowance, in any Node of the subject's Tenant. It belongs to one subject in one Tenant, is never shared with other subjects, never serves a Run that arrived through Federation or Tenant maintenance, and is not a Provider Credential. A subject that more than one External Identity is mapped to cannot hold one, and only the External Identity that connected it may use it. No Data Protection Level governs it; a Tenant may forbid Plan Connections, and then no Run whose delegation chain carries that Tenant's content uses one, nor does a chain that includes a Tenant on another Deployment.
_Avoid_: ChatGPT credential, subscription key, ChatGPT login

**Plan Token**:
An access or refresh token held by a Plan Connection. Aidash obtains one only by exchanging an authorization code it requested itself, or by refreshing; a Plan Token is never typed, pasted or uploaded into Aidash. It is kept in the Secret Store, is never returned or logged, and is not Key Material.
_Avoid_: ChatGPT token, plan credential, Key Material

**Tenant Secret**:
A named value a Tenant stores so that its own tool definitions can send it to the services they call, and only to the hosts recorded with it. It can be written and replaced but never read back, and it is neither Key Material nor a Provider Credential. On Cloud and Dedicated only the Credential Broker ever reads it, and its hosts are sealed together with each value, so changing where it may go there means entering the value again.
_Avoid_: Secret Vault, vault entry, tool secret, credential (unqualified)

**Deployment secret**:
A secret supplied in a Deployment's own settings. It is sent only to endpoints those settings name; no tool definition, built-in or a Tenant's, can name one.
_Avoid_: tool secret, environment secret, Tenant Secret

### Identity and sign-in

**External Identity**:
A person as known to the configured issuer and, for GCIP, to one GCIP Tenant. The same person in two GCIP Tenants is two External Identities.
_Avoid_: account, user, login

**Account Status**:
The issuer-reported state of an External Identity: whether it is disabled, and since when its sign-ins remain valid.
_Avoid_: user status

**Display Attributes**:
The verified email address and display name an issuer reports for an External Identity. Only Deployment Administrators, and the Tenant Administrators of a Tenant the person chose, see them, solely to recognize the person; they never grant or decide authority. A person chose a Tenant by addressing a Registration Request to it, through a Tenant Binding or an Invitation, or by creating it.
_Avoid_: profile, PII, claims

**Display Erasure**:
A Deployment Administrator's permanent removal of an External Identity's Display Attributes: later sign-ins no longer record them. The External Identity, its Mappings and its authorization history remain.
_Avoid_: deletion, anonymization, GDPR delete

**MFA Requirement**:
The declared second-factor rule of one GCIP Tenant, enforced by the issuer and never read by Aidash as authority. A GCIP Tenant with federated identity providers has none; its identity provider performs MFA.
_Avoid_: MFA claim, 2FA setting, MFA policy

**Registration Request**:
An External Identity's pending request to be approved into a Tenant. Where a Tenant Binding exists, the only Tenant it can be approved into is the bound one. A request made through an Invitation is addressed to the inviting Tenant. An addressed request is approved only into its Tenant; a request addressed to no Tenant is decided only by Deployment Administrators.
_Avoid_: signup, invitation

**Invitation**:
A single-use, expiring link a Tenant Administrator or a Deployment Administrator issues so that whoever opens it can make a Registration Request addressed to that Tenant. Opening it spends nothing; the first request made with it spends it, whatever that request's outcome. It may propose Assignable Groups. Unless its issuer chose that opening it joins directly, an administrator still approves the request and chooses the groups; when they did, the request counts as approved by the issuer, provided the issuer can still approve it then.
_Avoid_: invite code, join link, signup

**Mapping**:
An approved link from an External Identity to one subject in one Tenant, approved by a Deployment Administrator or by a Tenant Administrator of that Tenant.
_Avoid_: binding, membership

**Demo Reservation**:
A promise, made only by loading Demo data, that the first External Identity signing in with a given verified email address is mapped to a named subject of a Demo data Tenant, as a Tenant Administrator or a member. When exactly one such External Identity already exists, loading maps it at once instead. It never reaches an External Identity whose GCIP Tenant has a Tenant Binding, and it adds only the Mapping: a Personal Tenant is still provisioned as the Tenancy Rules say. It is the only way an email address ever leads to a Mapping, and a Deployment that does not load Demo data holds none.
_Avoid_: pre-mapping, email mapping, invitation (a link that only leads to a Registration Request)

**Deployment Administrator**:
A deployment-wide administrator who creates Tenants, approves Registration Requests, issues Invitations for any Tenant, and manages Mappings and Deployment Administrator grants. That authority is never scoped to a Tenant, and it never reads or changes a Tenant's content or settings: for that it acts through its own Mapping or a Subject Credential, under the Tenant's policy. Its only content-free reach into a Tenant's work is stopping a known Run.
_Avoid_: Operator (former name), System Administrator, sysadmin, admin, scoped Deployment Administrator

**Safety Stop**:
A Deployment Administrator's pause or cancellation of one Run that this Deployment executes, named by its Tenant and Run, made without seeing anything the Run contains and always with a reason. It is the only way a Deployment Administrator controls a Tenant's Run: it never resumes one, and a Run it paused stays paused until someone with the Tenant's own authority resumes it. The Tenant's people see that a Deployment Administrator stopped the Run, and those who may read the Tenant's audit history also see who did it and why.
_Avoid_: emergency stop, kill, Run management (the route's name, which Tenant subjects also use)

**Run Control**:
A person's pause, resume or cancellation of a Run under their Tenant's own authority, or their declining the call it paused on. Pausing and cancelling never depend on what the Run has read and show nothing it contains; resuming and declining do depend on it, and resuming also on the Use Recheck.
_Avoid_: Run management (the route's name)

**Authority Change**:
A change that returns only once nothing admitted under the old rules is still running: a change of policy, Catalog Approvals, Marketplace management or Generation Policies, a Subject Credential's revocation, a Tenant's deletion and the like. While one waits for that work to finish, new work in the Tenant waits before it starts, for a limited time. Stopping work, and finishing or checking work already begun, do not wait. A Node or service that does not answer is not an Authority Change: only a confirmed narrowing stops a Run.
_Avoid_: policy write, exclusive holder, revocation (for anything but a Provider Credential)

**Policy Busy**:
The refusal of an Authority Change because work already running did not finish within the limit. Nothing was applied, and the same request can be sent again. For a short while afterwards, further Authority Changes in that Tenant are refused at once, so repeated attempts cannot keep the Tenant's new work waiting.
_Avoid_: lock timeout, transaction pending

**Tenant Administrator**:
An External Identity mapped to a subject that its Tenant's policy permits to decide Registration Requests addressed to that Tenant, issue its Invitations, create user subjects for approved requests, place user and agent subjects in Assignable Groups, disable that Tenant's Mappings, and read and replace that Tenant's policy. It needs no Tenant Binding, and it remains an ordinary user of the Tenant through the same Mapping. Aidash refuses any change by a Tenant's own members that would leave it without a Tenant Administrator.
_Avoid_: tenant admin, tenant operator, owner, scoped Deployment Administrator

**Assignable Group**:
A group that a Tenant's policy marks as one Tenant Administrators may add user and agent subjects to or remove them from without replacing the policy. It is the only authority a Tenant Administrator can confer without permission to replace the policy.
_Avoid_: admin group, delegable role

**Identity Disablement**:
Marking an External Identity unusable: its sessions are revoked, its work pauses, and only a Deployment Administrator restore permits a fresh sign-in.
_Avoid_: ban, lockout

**Session Revocation**:
Ending browser or desktop sessions without affecting work already admitted on the External Identity's behalf. Unrelated to the Revocation of a Provider Credential.
_Avoid_: logout (when issued by the issuer rather than the user)

**Credential Broker**:
The service that, on Cloud and Dedicated, alone reads the Secret Store and attaches what it reads on Aidash's behalf: Key Material to provider calls, and Tenant Secrets to Tenant tool calls and to Egress gateway requests for External services. It holds the two under separate identities, so that reaching one never reaches the other. It sends a value only to the destination its Capability Token names, and never takes a value, destination or Tenant from the request. Each Dedicated deployment has its own Store and Credential Broker; Self-hosted nodes have neither.
_Avoid_: Proxy, gateway, key service

**Capability Token**:
A short-lived, signed grant that lets its holder have the Credential Broker use one stored value until it expires, without ever holding the value. A provider Capability Token is issued for one provider call and names one Provider Credential; a Tenant Secret Capability Token names one Tenant Secret at one version, the destination it may go to and the methods allowed there. Neither kind is ever accepted as the other.
_Avoid_: Broker token, access token, Subject Credential

**Token Subject**:
What a Capability Token is issued for: either one provider call a Run makes (an inference call, or a Decision Attempt), or a Tenant-scoped maintenance purpose such as memory indexing.
_Avoid_: Caller, principal

### Billing

**Billing Account**:
Who pays: an account owned by one External Identity, possibly acting for an organization, which can pay for several Tenants and buys Marketplace Agent Plugins for them. A Tenant is attached to or moved between Billing Accounts only with the consent of both a Tenant Administrator of that Tenant and the receiving account's owner.
_Avoid_: customer, payer, wallet

**Billing Contact**:
The email address the payment processor collects when a Billing Account subscribes, to which the processor sends receipts and payment notices; Aidash itself sends it nothing. It is kept for billing only, apart from Display Attributes, and it never identifies anyone or grants authority.
_Avoid_: verified email, Display Attributes, account email

**Subscription**:
A Billing Account's contract for one Tier, with its renewal and metered overage. Tenants are covered by a Subscription.
_Avoid_: plan (collides with Plan Connection), license

**Usage Credit**:
Prepaid usage, such as Execution Pod time and Egress gateway traffic, that a Billing Account buys in advance and that is drawn down as Tenants use it. When it runs out, new Runs and Sandboxed tools stop; reading and export continue.
_Avoid_: points, tokens (collides with model tokens), balance

**Tier**:
A named set of limits a Subscription grants, such as Tenants, Nodes, included Execution Pod time and storage. The stricter of the Tier and the Tenancy Rules applies. On Cloud, only a Personal Tenant whose sole member is its Tenant Creator needs no Subscription; it gets the Free Tier.
_Avoid_: plan, edition, level

### Deployment and execution

**Task**:
One piece of requested work in a Workspace, such as "total last month's sales sheet". A Task may have a parent and depend on other Tasks; it is completed, failed or abandoned as a whole.
_Avoid_: job, ticket, request (unqualified)

**Unassigned Task**:
An open Task that nobody is working on or can start: none of its Runs is under way, no other Node has taken it and no generated specialist awaits approval for it, or it has been handed over but waits on another Unassigned Task. When only Unassigned Tasks are left under a Run's Task, the Run tells its model about each one once. If the model gives a final answer again, those created by Runs of that Task are abandoned as it completes, and a person is asked about any others.
_Avoid_: orphan Task, unstarted child, undelegated Task

**Run**:
One attempt by one agent version at one Task. Authority, budgets, cancellation and recovery all apply per Run. A Task can have several Runs, such as a follow-up after new input or a delegated attempt elsewhere. A Run is not tied to any container: the place its commands execute outlives it.
_Avoid_: session, job, execution (unqualified), container

**Sandboxed**:
The tier of tools that act outside Aidash (shell, code, package installs, outbound requests, patches, file sharing). They are built-in definitions, yet each needs a Catalog Approval before a Tenant uses it, through which the Tenant decides where they may be used. The stdio MCP servers of Agent Plugins are Sandboxed too, enabled by approving one built-in group rather than as definitions of their own. They run in an Execution Pod from the Deployment's runner; the Deployment sets the limits, and a Deployment without a runner offers none.
_Avoid_: Host tier (its former name), host tools

**Test Limits**:
The bounds on a Tenant's Workbench tests, such as how many run at once and how many tokens, steps and seconds each may use. A Tenant sets its own within the ceilings the Deployment sets, and a lowered ceiling applies to values already set.
_Avoid_: test budget, test quota, Tenancy Rules

**Offering**:
How Aidash is provided: Self-hosted, Cloud or Dedicated. One deployment runs exactly one Offering, which fixes who operates it and the defaults of its tenancy rules.
_Avoid_: edition (collides with the Rust edition), tier, plan, distribution

**Community**:
The Apache-2.0 licensed part of Aidash, complete enough to run a Self-hosted Deployment on its own.
_Avoid_: open-source edition, free tier

**Enterprise**:
The BUSL-1.1 licensed part of Aidash that exists only to run the Offerings Aidash operates, Cloud and Dedicated: their cloud adapters, the Credential Broker, billing and operations tooling. Its source is public so that anyone can read it and report bugs; Self-hosted Deployments do not use it, and only Aidash runs it in production.
_Avoid_: Dedicated (the Offering), premium, pro

**Community build**:
An artifact made from Community alone, such as a binary, container image, UI bundle or source export. Self-hosted Deployments run it.
_Avoid_: community edition, open-source build, edition (collides with the Rust edition)

**Enterprise build**:
An artifact that also contains Enterprise. Cloud and Dedicated Deployments run it.
_Avoid_: enterprise edition, cloud build, edition (collides with the Rust edition)

**Change Date**:
The date on which one Enterprise version, meaning every snapshot and release of one `x.y.z`, becomes available under Apache-2.0. It is fixed four years after that version first carries Enterprise code publicly and never moves later.
_Avoid_: expiry, conversion date

**Self-hosted**:
The Offering whose Deployment the user runs themselves, under the Community license. Its Deployment Administrators are whoever runs it.
_Avoid_: on-prem, community edition

**Cloud**:
The Offering in which Aidash runs one shared, multi-Tenant Deployment. Only Aidash staff are its Deployment Administrators.
_Avoid_: SaaS (unqualified), hosted

**Dedicated**:
The Offering in which Aidash runs a separate deployment for one customer on infrastructure Aidash controls. The customer's administrators are its Deployment Administrators; Aidash staff keep the infrastructure and only break-glass recovery.
_Avoid_: Enterprise (the license, not the Offering), single-tenant, private cloud

**Deployment**:
One running Aidash installation with its own identity, Deployment Key and database. It holds many Tenants and runs exactly one Offering.
_Avoid_: node (now a Tenant's agent hub), instance, server, cluster

**Node**:
An agent hub inside one Tenant, with its own identity. Its agent and service subjects, Workspaces and Runs live in it, and all execution happens inside a Node. People are not owned by a Node: a user subject belongs to the Tenant and may work in any of its Nodes. A Node talks to any other Node, in the same Tenant, another Tenant or another Deployment, the same way: the receiver's authority decides. Across Tenants the receiving Node accepts only Nodes its Tenant Administrators registered; within one Tenant the Tenant's policy decides without registration. Every Tenant gets a default Node when it is created, and its Tenant Administrators may add more. An agent cannot create Nodes; a Subnode it creates is not a Node.
_Avoid_: cluster node (a machine), Deployment, server, instance

**Node name**:
The identifier of a Node, assigned by Aidash in the same form as a Tenant name, unique within its Deployment, never changed and never reused. A Node's address combines its Deployment's identity with its Node name and never reveals its Tenant.
_Avoid_: node ID, Node label

**Node label**:
The human-readable name of a Node, chosen and changed freely by its Tenant Administrators. It never identifies or routes to a Node.
_Avoid_: Node name, display name

**Node Deletion**:
A recorded decision to delete a Node. From that moment nobody can use the Node, and its running and queued work is cancelled rather than paused; it stops counting toward the Tenant's Node limit at once. Until its grace period ends, fixed when the deletion is recorded, its Tenant Administrators can restore it if the Tenant has room for it, which brings back its data and configuration under the Tenant's current policy but not cancelled work. Afterwards, or earlier when a Deployment Administrator purges it, its data is purged while its Node name stays retired. A Tenant's default Node and its last Node cannot be deleted.
_Avoid_: Node removal, decommission, soft delete

**Deployment Key**:
The signing key with which a Deployment vouches for its own Nodes in Federation. Its public part belongs to the Deployment's published identity. A Node Registration trusts the key it was registered with and follows each Key Rollover that key signs.
_Avoid_: Peer Credential (removed), node key, federation secret

**Key Rollover**:
A planned change of a Deployment's Deployment Key, announced by a transition the current key signs. Receivers that trust the current key accept the successor without action; the Deployment signs with the successor after a day, and the old key stops being accepted a week later. A receiver that missed that week must confirm the successor by hand. Only one Key Rollover runs at a time.
_Avoid_: key rotation, Key Replacement

**Key Replacement**:
A change of Deployment Key after the old key is lost or suspected compromised. The old key cannot vouch for its successor, so each receiving Node's administrators must confirm the new key themselves; until they do, that receiver does not trust it.
_Avoid_: Key Rollover, key rotation, revocation (unqualified)

**Federation**:
Communication between Nodes. It does not matter whether the two Nodes share a Tenant or a Deployment.
_Avoid_: peering (unqualified), cross-tenant sharing

**Remote Grant**:
A permission for one agent version on another Node to run one Task, carrying the Home's Federation Tool Request. The asking Node creates it from the Run's delegation chain and its Tenant's policy, whether a person or an Agent asks, and the receiving Node decides by its own Catalog Approval and authority whether to take it. It lasts one hour; once its own Run has started (its activation is recorded), it stays live past that hour while its Task is open and the receiver's agent still owns it, however long that Run's Stints sleep, and it ends with the Task or when the delegation chain breaks; the receiving Node may revoke it at any time (#1013). A Run that a Remote Grant started may hand work on to a further Node: every hop is a Remote Grant from the Task's Home Node, judged by the Home's policy over the whole chain (#654).
_Avoid_: remote delegation (unqualified), peer grant, federation grant

**Tool Reach**:
What a tool touches, which decides whether and where a Run started by a Remote Grant could use it: the Run itself (always usable), the Home's Workspace, the receiving Agent version's own material, the receiving Node's Agents, people, external services, an execution environment, or never. Only the kinds between the first and the last can be listed in a Federation Tool Request or Federation Tool Allowance.
_Avoid_: remote exposure (former boolean), tool class

**Federation Tool Request**:
The tools, by name or pattern, and the hosts that one Node asks another Node to let the Runs it starts there use. Its people with the right permission keep it per registered Node of another Tenant they send work to, and each Remote Grant carries it. Between Nodes of one Tenant there is none: the Tenant's policy decides.
_Avoid_: tool allowlist, requested scopes

**Federation Tool Allowance**:
The tools, by name or pattern, that a receiving Node lets Runs started by one registered Node of another Tenant use. Such a Run may use a tool only when both this and the Home's Federation Tool Request name it; a call that lacks either stops the Run, without a time limit, and asks the side that lacks it, whose people may allow it for that Run, add it, decline the call or block it. Only that other Node can look it up, after authenticating.
_Avoid_: tool allowlist, Federation Tool Approval, permitted tools

**Federation Tool Block**:
The tools, by name or pattern, that a receiving Node never lets Runs started by one other Node's Remote Grants use and never asks its people about. It wins over the Federation Tool Allowance and is never shown to the other Node.
_Avoid_: blacklist, deny list

**Home**:
The Node that created a Remote Grant. Its people follow and control the Run the grant started on the other Node; the other Node's people only pause and cancel it, and answer the asks about that Node's Federation Tool Allowance.
_Avoid_: source node, asking node

**Requester**:
The person a Remote Grant acts for, whether they asked themselves or their Agent asked. It is the same person the started Run acts for. Others on the Home may hold authority over that Run without being its Requester.
_Avoid_: owner, delegator

**Registry**:
The definitions of agents, models, tools, skills and other components that belong to one Tenant. There is no Registry shared between Tenants: a Tenant installs what it takes from the Marketplace or from a file as its own copy, and only built-in definitions are common to a Deployment.
_Avoid_: shared registry, global registry, catalog

**Built-in definition**:
A definition the Deployment provides to every Tenant, one Aidash itself defines and that works inside Aidash or inside an Execution Pod. No Tenant can change it, no Tenant definition can take its identifier, and it may be used without a Catalog Approval unless it is Sandboxed. A tool whose work is to reach something outside, such as reading a Web page, is a Preinstalled tool instead.
_Avoid_: system entry, core definition, host package

**Preinstalled tool**:
A tool Aidash provides in every Tenant's Registry whose work is to reach something outside the Deployment, such as a Web page, a model provider's own tool or a service like GitHub. It is not a Built-in definition. It is listed from the start but does nothing until the Tenant gives it a Catalog Approval, and, where the outside service needs a sign-in, until the person whose Run uses it connects their own account. A Tenant cannot delete one, only hide it.
_Avoid_: built-in tool (works inside Aidash), default tool (bound implicitly), bundled tool, stock tool, Marketplace Listing

**Tenant Configurator**:
The Built-in agent that changes a Tenant's settings to fit what a Tenant Administrator says they want. Only a Tenant Administrator starts it, and it acts as that person: it may do whatever they may, including approvals and policy changes no other Agent makes, except purging a Tenant or a Node.
_Avoid_: setup wizard, admin agent, configuration bot

**Subnode**:
A group of agents that an agent creates inside its own Node to carry out one part of a larger Task, lasting until its creator dissolves it or the creating Run ends. It is not a Node: it has no identity or address of its own. Its members are agent versions approved on that Node, the creator's own agent among them if it chooses, and one of them, its leader, runs every Task given to the Subnode. Its members may use only what the creating Run, the Subnode's allowance and their own definitions all permit, and hand work only to each other, though they may write to the agent that created it. The creator and the members alike may add approved agents to it or remove idle ones. It has its own Workspace, which the creating Run may read and which stays readable with the creating Run's records once the Subnode ends. Once it ends, nothing new can be written to that Workspace, and no Agent can write to its Memory or ask a person anything from it. A Run that was working there can still end, and a person can still correct or delete that Memory. A Read Recheck of the creating Run crosses into that Workspace, though only the creating Run reads it directly. A Task the creating Run gives the Subnode reaches its members without a Read Recheck of what that Run had read, so withdrawing what that Run read does not stop them; anyone else who opens it is still checked. The other way does not hold: what a member writes to the creating Run stays bound to what that member read, so withdrawing it hides the message from everyone and stops the creating Run that received it. A Subnode never creates another Subnode. A Subnode Task may carry a range of the creating Run's Context Journal, which counts as that Run's read and which the members read only through Journal Query.
_Avoid_: Crew, team, Node (a Subnode has no identity)

**Agent Plugin**:
A package in the open Agent Plugins format, carrying Skills and MCP servers, that an Agent version holds as a pinned copy, together with the exact packages its stdio MCP servers need, so that only that Agent's Runs can use them. It comes from a GitHub repository or from the Marketplace. It adds no definition of its own to the Tenant's Registry, and a changed copy needs a new Agent version.
_Avoid_: plugin (unqualified)

**Agent Plugin data**:
The files an Agent Plugin's stdio MCP servers keep between Runs, one set per Scratch Space, Agent and Agent Plugin. Everyone allowed to use that Scratch Space shares it, as they share the space's content, and it survives new Agent versions and the space's wipes. It is never part of a Working Area. It disappears with its Scratch Space or its Agent, and anyone allowed to use the space can clear it for one Agent Plugin.
_Avoid_: plugin state (also used for session memory), plugin data (unqualified)

**Marketplace**:
The one place, operated by Aidash, where Publishers offer Listings that set up or extend a Tenant to every Deployment connected to it. A Self-hosted Deployment is connected only if its Deployment Administrators connect it, and nothing installed from it needs it to keep running. Tenants share definitions with each other only through it or through a file.
_Avoid_: store, app store, plugin registry, Shared Package (removed)

**Listing**:
One thing a Publisher offers on the Marketplace, free or for sale, publicly or only to the Tenants and people it names: a Registry definition, an Agent Plugin, or a Pack. Aidash checks it automatically and does not review it by hand.
_Avoid_: Merchandise, product (Aidash itself), item, offer, Offering

**Pack**:
A Listing that lists other Listings, possibly by other Publishers, that a Tenant installs together. It carries no content of its own, has no price, names no versions and never becomes a Registry definition: each item installs, updates, is approved and is paid for on its own.
_Avoid_: bundle (a Registry definition grouping tools and skills), extension pack, set

**Publisher**:
An account on the Marketplace that offers Listings and receives what they earn. Aidash itself is one. A Publisher is not a Tenant.
_Avoid_: owner Tenant, vendor, developer, seller

**Declared Permissions**:
What one version of a Listing may reach and use: destinations, Tenant Secrets, providers, the packages of its stdio MCP servers, and sampling or elicitation. The Marketplace derives them from a definition; only for an Agent Plugin does its Publisher declare them. Aidash lets that version reach and use nothing else, and what a remote server does with what it receives is only disclosed.
_Avoid_: permissions (unqualified), scopes, manifest permissions

**Entitlement**:
The right of one Tenant or one person to use one Listing, either without end or until a date. A Billing Account pays for it unless the Listing is free.
_Avoid_: license (the software license), purchase, Subscription (a Billing Account's Tier contract)

**Recall**:
Aidash's forced stop of one version of a Listing: no Deployment connected to the Marketplace starts a new Run of an Agent version that carries it or depends on it, directly or through other definitions.
_Avoid_: Revocation (of a Provider Credential), takedown, delisting

**Library**:
The Listings that one Tenant or one person holds an Entitlement to. A Tenant Administrator manages a Tenant's Library. Listings from a person's Library may be used in a Tenant other than a Personal Tenant only if the Tenant's policy allows it, which it does not by default.
_Avoid_: purchases, collection, installed plugins, installed Listings

**Marketplace Connection**:
A Deployment's registration with the Marketplace, through which its Tenants browse it, obtain Listings, publish Listings and receive Recalls. A Cloud Deployment always has one; on Dedicated and Self-hosted only the Deployment Administrators create it.
_Avoid_: marketplace sync, Plan Connection

**Marketplace Account Link**:
A Tenant's or a person's authorized link, through a Marketplace Connection, to an account on Aidash Cloud, so that a Tenant can be attached to a Billing Account there and a person can use their own Library. Signing in to the Deployment itself never goes through it.
_Avoid_: account linking (unqualified), Sign in with Aidash

**Deployment-hosted model**:
A Built-in definition of a model or embedding model, whose model and version (and, for an embedding model, dimensions) Aidash fixes, that the Deployment Administrators run inside their Deployment. It is called as a Keyless endpoint, nothing sent to it leaves the Deployment, and a Deployment that does not run it offers none. It is never a failover target for another model.
_Avoid_: self-hosted model, self-hosted embeddings, local model, in-deployment model, built-in model

**Reference Document**:
A text or an image registered together with one agent version as that agent's private knowledge. The agent's definition carries only its digest, so discovering or reading the agent never reveals its content.
_Avoid_: personal agent (for an agent registered with them; collides with Personal Tenant), knowledge file, attachment

**Cluster Node**:
One machine in a Kubernetes cluster that hosts Pods. It carries no Aidash identity and may be replaced at any time.
_Avoid_: node (unqualified), worker node, VM, host

**Environment**:
One nonproduction Deployment, such as develop, pr-N or test, with its own public origin and retained data.
_Avoid_: stage, cluster

**Demo data**:
A set of Tenants, subjects, Registry contents and Catalog Approvals loaded once into an Environment or a local development Deployment so that people can try Aidash. It is never loaded into a production Deployment. It holds no Provider Credentials: a person connects one through Provider Authorization as usual. An Environment built from a fork never receives the real people Demo data names.
_Avoid_: seed or seed data (writing Built-in definitions into every Registry), fixture (test data), sample data, preview environment

**Execution Pod**:
The one gVisor-isolated Pod of a Scratch Space, from the image its settings name, in which every command, interpreter session, stdio MCP server and Live preview of the Agents using that Scratch Space runs. It keeps running between commands and Runs and stops after going unused for a while, though not while a Background process started with a time limit is still within it; its processes may outlive the Run that started them. It can reach nothing but the Egress gateway, nothing but the Preview gateway reaches it, and no Tenant Secret ever enters it.
_Avoid_: sandbox (unqualified), runner pod, container, workstation

**Background process**:
A process that an Agent's command leaves running in the Execution Pod after the command returns, whether started on purpose or left behind by a command that finished. One started on purpose has a time limit and keeps its Execution Pod running until it ends or the limit passes; one left behind has no limit and keeps nothing running. It belongs to the Run that started it, is listed with that Run, and its output can be read later. A person or an Agent can stop it, and it stops when that Run is cancelled or its authority to read what it read is revoked, but not when the Run completes.
_Avoid_: daemon, detached job, orphan process

**External service**:
A definition of one secure Web address with a Tenant Secret that Sandboxed code may reach through the Egress gateway, with the methods it allows; the secret is attached on the way. A Tenant defines its own and makes it available to a Scratch Space, whose users' Agents may then all use it; no Agent definition names one, and only code in an Execution Pod can reach one. It is usable there only through a Catalog Approval that covers the Scratch Space's Node and is not limited to certain users or Agents, and always in the newest version so approved. Withdrawing that approval, or taking the service off the Scratch Space, also stops requests already under way. Approving one admits anything the code sends to it.
_Avoid_: egress allowlist, allowed domain, proxy target, Agent's External service binding

**Egress gateway**:
The only destination an Execution Pod can reach. It lets code reach the hosts its Scratch Space allows, recording the host and the bytes, and forwards requests to the External services available to that Scratch Space, attaching their Tenant Secrets and recording each request.
_Avoid_: proxy (unqualified), NAT, sandbox network

**Host connection**:
One connection that Sandboxed code makes through the Egress gateway to a host its Scratch Space allows. Aidash records its host, its bytes and the Run whose code made it, not its contents, and attaches no Tenant Secret, unlike a request to an External service. The Run is what the code says it is: code in a shared Execution Pod can claim another Run, so the record is not a security boundary.
_Avoid_: proxy connection, tunnel, HTTPS proxy

**Egress fair share**:
The share of Host connections and External service requests that each Tenant with open connections gets while an Egress gateway runs short of open files or memory: an equal part of what is open. Otherwise nothing caps them. A new connection over its Tenant's share waits until a shortage of open files ends, and its wait is recorded; during a shortage of memory it is refused instead, because a waiting connection holds memory itself.
_Avoid_: connection limit, tunnel limit, connection quota, rate limit

**Scratch Space egress allowance**:
An optional cap, set in a Scratch Space's settings and absent by default, on the bytes its code may send, and on those it may receive, through the Egress gateway in one day, counted apart. It covers both its Host connections and its requests to External services; an External service may have a smaller share of its own. A slow store does not count as running out of it.
_Avoid_: budget (an Operation's budget is another thing), quota

**Egress IP group**:
The Tenants whose traffic leaves the Egress gateway from the same public addresses. Whoever runs the Deployment assigns Tenants to groups; a Tenant not assigned shares the default group.
_Avoid_: NAT pool, egress pool

**Egress request record**:
The record of one request that Sandboxed code makes through the Egress gateway to an External service. Its status is the one the client received, not the one the upstream answered. Like a Host connection's, the Run it names is the one the code says it is.
_Avoid_: egress log, audit entry

**Retention Rules**:
The rules for how long Aidash keeps content it holds only for a while. The deployment-wide periods are the same in every Offering unless whoever runs the deployment sets other finite periods, and they are also the longest any Tenant may keep. A Tenant may keep its own shorter periods. A change applies to everything already kept, not only to what is kept afterwards.
_Avoid_: retention policy (collides with a memory policy's retention), TTL

**Lease Recovery**:
Another worker taking over a Run because its previous lease expired without being released, whether the process running it died, stalled, was stopped mid-step or was cut off from the database. A worker that gets its own expired lease back before anyone else takes the Run, such as after a database outage, has not had its Run recovered. Consecutive Lease Recoveries are counted until one of the Run's steps is saved.
_Avoid_: retry (a step that returned an error), restart, reclaim

**Isolated Step**:
The step after a Lease Recovery, run with nothing else in flight in its process, so that a process death during it can be attributed to that Run alone.
_Avoid_: quarantine, exclusive run

**Crash Loop Failure**:
The terminal failure of a Run whose step never got saved across a bounded number of consecutive Lease Recoveries. The Run is not handed out again, and its failure says the worker kept stopping on that step.
_Avoid_: crash (unqualified), poison Run, retries exhausted

### Preview

**File version**:
One unchangeable content of one path in a Working Area. Changing the path makes a new File version; a File preview and a correction request point at one.
_Avoid_: revision (counts a Working Area's publications), file (unqualified when the version matters), snapshot

**Retained version**:
A File version that something refers to, kept after a later publication replaces it until the last reference goes, its Working Area is cleaned up or its Tenant is deleted. When an HTML File version is referred to, the File versions of the rest of its Working Area at that revision are retained with it.
_Avoid_: snapshot, history, backup, pinned file

**File preview**:
The dashboard's display of one exact File version as a document rather than as bytes.
_Avoid_: preview (unqualified), artifact preview, rendered file

**Live preview**:
A person's view, in their own browser, of a Web server that Sandboxed code runs in an Execution Pod. It belongs to the Run that started it and outlives that Run's completion only within its limits.
_Avoid_: preview (unqualified), preview environment (an Environment), port forward, tunnel

**Preview gateway**:
The only way into an Execution Pod. It admits a person allowed to view a Live preview and forwards their browser's requests to that preview's Web server.
_Avoid_: ingress, reverse proxy (unqualified), tunnel

### Model-facing context

**Projection Version**:
The immutable rule for rendering a Context Projection into the bytes of a model request. Native is the only one; Legacy and Ordered were removed, and a definition that names none means Native. An Agent definition names it, it is copied into the Run's Binding snapshot, and it never changes during the Run. It is separate from the State Version, which versions persisted state.
_Avoid_: prompt version, layout version, protocol version

**Harness Contract**:
The versioned text Aidash itself gives every Agent's model about how Aidash works: which content instructs and which is data, what completes a task, and where tool aliases are defined. An Agent definition names it, it is copied into the Run's Binding snapshot, and each version declares the Projection Versions it can be rendered with. Agent definition instructions and Skill bodies are not part of it.
_Avoid_: system prompt, boilerplate, prompt version, base prompt

**Instruction precedence**:
The order in which a Run's model resolves conflicting directions: the Harness Contract, then human answers, then the Agent definition's instructions and Skills, then the task and run messages, then Repository rules, then data. It shapes only how work is done; runtime authorization alone decides what may execute.
_Avoid_: priority, override order, trust order

**Model Turn**:
One Accepted Response together with every tool call it made and the result of each call. A Model Turn is summarized and rendered as a unit, never partially; only its results may be offloaded one by one. Every Run records its Model Turns in the Context Journal.
_Avoid_: round trip, step (a step can contain no Model Turn)

**Stable Prefix**:
The leading bytes of a model request that stay identical across the steps of one Run: the Tenant Cache Salt, instructions, the tool definitions in the Exposure set, Run-stable context and all earlier history. Changing the Exposure set or a resident Skill body ends it at that point.
_Avoid_: system prompt (it is only part of the Stable Prefix), cached prompt

**Cache Scope**:
The set of requests that may share a provider's cached prefix. It is never wider than one Tenant.
_Avoid_: cache key, cache namespace

**Tenant Cache Salt**:
The first line of `system` in every request of a Run: a truncated HMAC of the Tenant ID under a Cache Salt Key, labeled with that key's version. Only the transport adapter computes it; it never appears in request metadata, logs, diagnostics or Usage Records.
_Avoid_: cache key, tenant hash, prompt salt

**Sticky-routing id**:
The opaque `session_id` every step of a Run sends, so OpenRouter keeps the Run on one provider and its prompt cache stays reachable. An HMAC of the Tenant and the Run under a Cache Salt Key, because it reaches OpenRouter's Broadcast destinations.
_Avoid_: session (a Run has no session), Run ID (the raw ID is never sent)

**Cache Salt Key**:
A versioned, node-configured secret from which Tenant Cache Salts are derived. It is not Key Material and never lives in the Secret Store. A node without one cannot create Runs; rotating it only causes cache misses.
_Avoid_: cache key, salt secret, Master Key

**Retrieval Key**:
The inputs that determine one Run's semantic retrieval: query inputs, the Run-fixed retrieval budget, binding digest, authorization scope and the authority, index and memory-binding revisions. While it is unchanged and current authority still permits the result, later steps reuse the same Source observation.
_Avoid_: cache key, retrieval cache key, boundary

**Outage Wait**:
A Run waiting, without holding a worker, for a service it needs that does not answer: the database, the Approval Judge, an embedding or vector service, an index still being built, another Node, or a used-up plan allowance. Attempts made while waiting count against no limit and no charge. The Run goes on by itself once the service answers; after 24 hours it pauses until a person resumes it. A confirmed Authority Change is never an Outage Wait.
_Avoid_: outage pause, dependency wait, service wait

**Degraded context**:
Optional Workspace semantic context that a local Run step went without, wholly or in part, because its retrieval stayed unavailable after the bounded retry or its index was not yet complete, and no source feeding it chose to wait instead; a typed unavailable marker naming the cause stands in for what is missing. A later step never reuses it. Required-Home semantic context and native memory never degrade.
_Avoid_: Fallback, skipped context, partial context

**Retry Cycle**:
The span of one semantic operation between authorized manual retries of its Run. Its retry ledger bounds automatic retries only within the current Retry Cycle; memoized results and the operation's durable call, token and cost budgets span every cycle.
_Avoid_: resume count, retry round, cycle (unqualified)

**Cache Mode**:
A model definition's declared prompt-caching behavior: none, automatic or explicit. Only an Agent definition that opts in, on a model declaring explicit, sends cache breakpoints.
_Avoid_: cache profile, caching support

**Request Estimate**:
Aidash's labeled, pinned prediction of a request's token size, with the estimator's identity and confidence. It is never a substitute for reported usage.
_Avoid_: token count, usage

**Estimate Snapshot**:
The estimator, its version, its Token Encoding if any, and the calibration ratio and margin that one Run step uses for all its Request Estimates. It is fixed before the step first relies on an estimate and is reused when the step resumes; a step whose Token Encoding the worker no longer has resumes on the conservative one. A calibrated or tokenized snapshot becomes the conservative one for the rest of the Run when a Context Overflow follows it, within the same step, or when the provider reports more input tokens than it estimated, from the next step.
_Avoid_: calibration snapshot, estimate cache

**Token Encoding**:
A code-defined rule, from a closed list, that turns text into the tokens of one model family, such as `o200k_base`. A model definition names one explicitly to have its requests counted token by token; it is never inferred from the model's identifier and is not a Registry definition. Counting covers only the text the model sees, so a tokenized Request Estimate is still an upper bound, not the provider's count.
_Avoid_: tokenizer (a retired Registry kind), vocabulary, exact estimate

**Usage Record**:
The append-only record of one Inference Attempt's provider-reported tokens, cache reads and writes, cost, rejection class and Request Estimate. Any value the provider did not report is unknown, never zero.
_Avoid_: usage (unqualified), billing record

### Inference

**Inference Attempt**:
One provider call, owned by exactly one Stint (for a step of its Run) or one Model Call. It has a public identifier that is separate from the worker's lease token, owns one usage reservation, and is settled at most once. A retry or a re-execution under a new lease is a new Inference Attempt of the same owner.
_Avoid_: attempt (unqualified), request, claim

**Model Call**:
One model request made for a purpose other than a step of a Run: a compaction summary, a Goal judgment, a memory role, a Workbench test or MCP sampling. It is sent in the Native form and is to these purposes what a Stint is to a Run's steps: the owner of their Inference Attempts. One made during a Run belongs to the Stint awake at the time and counts against that Run's Budgets; one made outside a Run belongs to its Tenant and Node. It records its purpose, owner, model version, a digest of what was sent (never the content), its outcome, usage and cost, and when it started and ended. A Compaction Attempt keeps its own record and names the Model Call it used; an Approval Judge request is a Decision Attempt.
_Avoid_: Call (unqualified; a Call is a tool call), auxiliary request, side call, Inference Attempt

**Accepted Response**:
The single complete model response of an Inference Attempt that passed the same validation as a non-streamed response and ended with an accepted finish reason. Only an Accepted Response enters ToolCall state or accepted context. A final Accepted Response without tool calls can trigger Goal judgment.
_Avoid_: final delta, completed stream

**Inference Progress**:
Bounded, sanitized, tentative display data from one Inference Attempt: text deltas and tool-call identity, name and argument size. It never carries partial tool arguments or provider reasoning, is never executed, never enters context and never becomes a Completion Message.
_Avoid_: partial response, delta journal, streaming output

**Progress Outcome**:
How an Inference Attempt's displayed progress ended: accepted (an Accepted Response replaced it), discarded (new user input superseded it), or interrupted (cancellation, lease loss, revoked authority or a failed stream). Until then it is pending. It is written exactly once and never changes; a later discard of the Accepted Response is a fact about the Run, not about the progress.
_Avoid_: stream status, delta state

### Capability exposure

**Exposure policy**:
The versioned rule, fixed with a Run's Binding snapshot, that decides which bound capabilities and Skill bodies a model request carries. An Agent definition that names none uses the legacy policy, which exposes every bound capability.
_Avoid_: Tool selection, prompt policy

**Discoverable capability**:
A capability in a Run's Binding snapshot that current authority still permits the Run's model to find and describe. Being discoverable does not place its definition in a model request. A default capability whose prerequisite the Binding snapshot lacks, such as memory tools without a memory provider, is excluded from the snapshot and is never discoverable, so it can cause neither a call nor an Authority Pause.
_Avoid_: Eligible binding, available tool

**Exposure set**:
The capability definitions and Skill bodies carried by one model request. Only a capability in the Exposure set can be invoked by its Model alias.
_Avoid_: Active tools, visible tools, tool list

**Load**:
A Run's model bringing a Discoverable capability into the Exposure set of its later requests. Loading never installs a package, widens authority, or changes the Binding snapshot.
_Avoid_: Activate, enable, install

**Unload**:
A Run's model removing a loaded capability or Skill body from the Exposure set of its later requests. Mandatory exposure cannot be unloaded.
_Avoid_: Deactivate, disable, evict

**Mandatory exposure**:
The capabilities a deferred Exposure policy always carries: the Required Bindings and the means to find, describe, load and unload capabilities. An Agent definition whose Mandatory exposure cannot fit its model is rejected rather than run with less.
_Avoid_: Core tools, always-on tools, mandatory envelope

**Eager binding**:
A Binding its Agent definition places in the Exposure set from a Run's first request under a deferred Exposure policy. Eagerness affects only exposure, never authority.
_Avoid_: Pinned tool, preloaded tool

### Context recovery

**Context Journal**:
The append-only, sequence-numbered record of every context event in a Run. It is the authoritative history; compaction never edits or removes from it.
_Avoid_: History log, transcript

**Context Projection**:
The lossy view of the Context Journal that a Run sends to its model: Offloaded Results in place of their content, an optional Execution Summary, and the verbatim recent tail. Message coverage, read dependencies and pending execution state never derive from it.
_Avoid_: Context (unqualified), saved context, prompt history

**Context Policy**:
The versioned rule, fixed with a Run's Binding snapshot, that decides how an oversized Context Projection is reduced: first by Offloaded Results, then by the Summary Stage, and the Run pauses only when neither makes the request fit. No model decides what is kept.
_Avoid_: Compaction settings, compactor config

**Summary Stage**:
The pipeline step that folds older Context Journal events into the Execution Summary with the Agent's own model, once Offloaded Results are not enough. Goal, people's words and tool outcomes are copied from the Context Journal rather than written by the model; an item without support is dropped, and an item of the previous summary is never dropped.
_Avoid_: Summarization fallback, auto-summary

**Execution Summary**:
The bounded, structured summary in a Context Projection. It records intent, constraints, decisions, unresolved work and references, the exact journal range it covers and that range's source dependencies.
_Avoid_: Summary (unqualified), run message summary

**Offloaded Result**:
A tool result that the Context Projection shows only as a marker naming its place in the Context Journal, either on arrival because it is too large or later to free room. The original stays in the Context Journal, and the Agent reads it back with Journal Query.
_Avoid_: pruned result, truncated result, elided result, cleared result

**Built-in Memory**:
The memory every Agent has without any setup. It learns facts from the Agent's Runs with the Agent's own model, finds them by keyword, time and the links between them, and keeps them without a person admitting them; contradictions are reconciled into observations. Each fact names the Run and the range of its Context Journal it came from, and what it recalls reaches the model as data. It is shared within one Workspace unless the Tenant widens it to the Agent across the Tenant. A person's deletion of a fact puts a Deletion Mark on the facts learned from Runs that recalled it, and tells the running Runs that read it; the memory's own evictions, corrections and reconciliations do neither.
_Avoid_: default memory, preinstalled memory, Hindsight (the upstream project)

**Deletion Mark**:
A note on a Built-in Memory fact saying that a Run it was learned from had recalled a fact a person later deleted, quoting the deleted fact. A marked fact stays recallable, ranks below unmarked ones and asks the model to rely on it only if it does not depend on the deleted fact. The mark spreads to facts learned from Runs that recalled a marked fact, and a new revision with new evidence clears it. Adding or clearing it is not a change for Memory Invalidation.
_Avoid_: stale (for this), withdrawn, unverified

**Memory Invalidation**:
What stops a Run whose read memory changed: a memory Unit it read gained a new revision, was withdrawn, deleted or expired, or its candidate's source did. Any real change counts, metadata included, and no writer's label exempts one; a Deletion Mark does not count. A write identical to the current revision is not a change. An invalidated Run cannot continue, not even with fresh context; the only way forward is a Follow-up Task.
_Avoid_: stale Run, source changed (unqualified)

**Follow-up Task**:
A new Task created after a Run stopped, carrying only the instructions its requester supplies. It has no parent or dependency link to the stopped Run, so no context, approval or verification from that Run carries over.
_Avoid_: retry Task, continuation

**Journal Query**:
Read-only code an Agent writes to search and read its own Run's Context Journal, or the range a creator handed to a Subnode Task. It sees revoked content as its replacement, cannot call tools, write anything or reach the network, and its output reaches the model as data.
_Avoid_: context_read, history search, REPL (unqualified)

**Compaction Attempt**:
One durable try at reducing a Context Projection. It is recorded before any provider I/O and is either adopted atomically with the Run's save or ends without changing the projection.
_Avoid_: Compaction run, compaction job

**Context Overflow**:
A provider's proof that a request exceeded its context window. Only a Context Overflow enters compact-and-retry. Output truncation, refusal and other rejections are not overflows.
_Avoid_: Context limit error, too-long error

### Decision layer

**Approval Judge**:
Aidash's built-in judge for Auto Approval Mode. It applies Aidash's fixed risk threshold to a pending approval, within the person's authority and the Tenant's policy.
_Avoid_: Decider (a retired Registry kind), tenant-selected judge

**Decision Attempt**:
One provider call the Approval Judge makes for a pending approval. Its provider-reported cost is kept in the Decision Evidence.
_Avoid_: Inference Attempt, Model Call

**Decision Evidence**:
The record of an Approval Judge decision, including the judge version, input digest, returned probabilities and ruling. It lets the Run replay the ruling without calling the judge again.
_Avoid_: Context Journal, audit entry

### Tool execution

**Concurrency safety**:
The provider-declared fact, part of a tool's contract, that its calls may share a Tool Batch. A tool is Sequential unless its provider declares otherwise; Registry configuration can only lower it. Read-only effect and replay safety never imply it.
_Avoid_: Parallel flag, parallelizable, read-only (as proof of independence)

**Resource Claim**:
A resource identity and access mode, Shared or Exclusive, that a provider derives from a call's validated arguments. A call whose claims cannot be derived runs alone.
_Avoid_: Resource target (authorization only), lock, resource (unqualified)

**Tool Batch**:
A contiguous run of tool calls from one model response, kept in the model's order, that execute together because each has Concurrency safety and no two hold conflicting Resource Claims. Calls are never reordered to form a batch. A batch is admitted durably before any of its calls starts and is adopted into the Context Journal as a whole, in call order; it never claims that its calls observed one consistent snapshot.
_Avoid_: Parallel group, wave, fan-out

**Tool Parallelism**:
The number of one Run's tool calls executing at once. It never exceeds the Run's pinned ceiling or the ceiling of the process executing the Run, and both default to one.
_Avoid_: Concurrency (unqualified), worker slots

**Tool call approval**:
An eligible person's authorization for one exact call of a Tenant's HTTP or MCP tool or of Page reading, which the Run waits for and then makes. It is decided by an Eligible approver and may be given once or as a Run grant. A call to a remote MCP server of an Agent Plugin needs one too; a call to its stdio MCP server does not, because that server is Sandboxed.
_Avoid_: External approval, workbench approval, external write approval

**Read-only operation**:
An operation of a Tenant's HTTP or MCP tool definition that the definition's version declares to change nothing outside Aidash, so its calls need no Tool call approval. What an MCP server says about its own tools never makes one, and it implies no Concurrency safety. A tool of an Agent Plugin's remote MCP server becomes one when the Agent version declares it together with the digest of the input schema it was declared against.
_Avoid_: Read-only hint, safe tool

**Rejected Call**:
A tool call that Aidash refuses or cannot carry out for a reason no change of authority on the Run's own Node could lift: its arguments are wrong, what it names is missing or not visible to the Run, the current state does not allow it, a boundary of the Run stands in the way, or another Node denied it. It comes back to the model as the call's error, saying what to change or that another way is needed, and whether the call may have partly happened, and the Run goes on. A refusal that a change of authority on that Node could lift is never one; that is an Authority Pause.
_Avoid_: denied call, forbidden call, authority error (for argument mistakes)

**Failed Call**:
A call of a Tenant's HTTP or MCP tool, or of an Agent Plugin's MCP server, whose service answered with a failure or that certainly never reached the service. It comes back to the model as the call's error, with what the service said, and the Run goes on. When the service failed after it may have started the change, the error says so. Unlike a Rejected Call, the answer comes from the service, not from Aidash.
_Avoid_: tool outage, external error

**Uncertain Call**:
A tool call that may have reached its service but whose outcome Aidash never learned, because the worker stopped or the answer was lost after the request was sent. A replay-safe tool is called again under the same idempotency key; any other tool waits for a person to reconcile what the service did, told which of the two happened.
_Avoid_: unfinished invocation, maybe-completed call

**Time Limit**:
How long a command or Python cell may run, counted from the moment it starts. Preparing its sandbox does not count.
_Avoid_: timeout, deadline (for the duration)

**Command Deadline**:
The moment a running command's Time Limit ends and Aidash stops it. A command that has not started has none.
_Avoid_: kill time, expiry

**Timed-out Command**:
A command or Python cell that Aidash stopped, with the stop confirmed, because it was still running at its Time Limit, whatever exit code it then gave. Its Run goes on. What it wrote to its Working Area is kept unless the save is refused; the rest of its sandbox is gone. Unlike an Unconfirmed Command, it is settled.
_Avoid_: killed command, expired command

**Unconfirmed Command**:
A command whose time limit has passed without Aidash being able to confirm that it stopped. Its Run goes on, its Working Area stays busy and it can no longer reach outside. Aidash keeps watching and settles it on its own once the stop is confirmed; nobody reconciles it, unlike an Uncertain Call.
_Avoid_: uncertain operation, lost command

**Authority Pause**:
A Run stopped because authority refuses a step: a policy denial or a missing Catalog Approval on its Node, for the Run or for the Agent it delegates to there, an agent setting or capability it lacks, another Node's policy asking for approval, or a Tenant Deletion. It names the tool or stage and the check that refused, and whose authority refused when it is not the Run's own, without the protected content, and resuming lets the step run once a person has changed that authority. A person may instead decline a tool call that certainly did not run, which makes it a Declined Call. A call whose arguments are wrong never causes one; that is a Rejected Call. A refusal that no change of authority could lift within the Run, because its Binding snapshot lacks what the step needs, must never cause one either.
_Avoid_: authorization blocked, execution authority denied (as a reason), Forbidden pause

**Declined Call**:
A tool call stopped by an Authority Pause that a person answers instead of lifting the refusal. It certainly did not run; it comes back to the model as the call's error, saying a person declined it and that another way is needed, and the Run goes on. Declining gives the Run no authority. It covers only that one call, so the same refusal stops the Run again, unless the person declined that tool's same kind of refusal for the rest of the Run; that never reaches other Runs, child Runs included. Unlike a Rejected Call, the refusal could have been lifted and a person chose not to.
_Avoid_: refused by a person, skipped call, cancelled call

**Provider tool**:
A tool that the model provider itself runs inside one model request, such as OpenRouter's web search or Fusion. Aidash neither executes it nor sees what it sends before it is sent. Only a Run whose model is called with the Tenant's Provider Credential for OpenRouter can use one, and no promise about training or retention covers it, so only a call held to No Restriction, after its delegation chain and the Deployment floor, can use one. Its Catalog Approval is the only consent it needs: once approved, a Run's model uses it without asking anyone.
_Avoid_: server tool (collides with MCP server), OpenRouter plugin, web flag, online model

### Approvals

**Approval Mode**:
Who decides, for one person's Runs, an approval they would otherwise wait for: Manual asks a person, Auto uses the Approval Judge, Don't ask refuses it so the Run never waits, and Bypass approves whatever the Tenant's policy lets that person approve. A person has a default and may switch it in any Thread at any time; the Tenant's policy decides which modes each subject may use.
_Avoid_: Permission mode (a permission is what the Tenant's policy allows), autonomy level, approval policy

**Approval Rule**:
One person's allow, ask or deny decision, made in advance, for one kind of approval and one target, such as a Tenant tool, an origin, a page host or a Generation policy. It covers only Runs whose delegation chain starts at that person; a deny holds in every Approval Mode, and an allow never reaches beyond what the Tenant's policy lets that person approve. Its target may be every target of its kind.
_Avoid_: Permission rule, Advance Approval, pre-approval, standing approval, allowlist

**Answer Deadline**:
How long one person's Runs wait for an answer to their model's question before continuing without one, on the model's own judgment. A person sets it for all their Runs, there is none by default, and it never applies to Completion Reviews, failed subtasks, Unassigned Tasks or reconciling an uncertain tool call.
_Avoid_: question timeout, Human Request timeout

**Human Request**:
A Run's request for a person's answer, such as its model's question, an approval its model asks for itself or a request for information, which the Run waits on until it is answered. Tool call approvals and Elicitations are not Human Requests.
_Avoid_: human question, prompt, approval card

**Sampling approval**:
An eligible person's authorization for one MCP server of an Agent Plugin to have the Run's model write one completion for it, without any of the Run's context. Like a Tool call approval it is decided by an Eligible approver and may be given once or as a Run grant, but the server's tool call stays open while it waits, and an expired one is refused to the server.
_Avoid_: sampling permission, model access approval, LLM request approval

**Elicitation**:
An MCP server's request, made while one of its tool calls is open, for a person in the Run's Thread to fill in a flat form. It is never an approval: it waits no longer than the shorter of the person's Answer Deadline and the approval expiry, and an unanswered one is declined to the server.
_Avoid_: Human Request, server question, form prompt

**Setting Request**:
A form the Tenant Configurator shows in its Thread for something only a person can do or should type: a secret for a tool, an OAuth consent, a payment, a purge, a choice or a short answer. Aidash checks every answer against the form before using it; a secret goes straight into the setting it names, and the model learns only the setting's name and whether it was saved. An unanswered one is handled like a Human Request.
_Avoid_: secret prompt, setup form, Elicitation (an MCP server's form)

**Task Model**:
The model and reasoning effort a Task runs with when its agent leaves the model unset. The person starting the Task picks it; a Task started by a Run inherits the creating Run's or takes the one it names, and a Duty keeps one for the Tasks it starts. Every Run of the Task uses it, and it must be a registered model approved for the whole Tenant.
_Avoid_: model override, default model, Tenant default model

### Web pages

**Page reading**:
A Preinstalled tool that fetches one public Web page, or one part of a long one, and gives the Run its text; a PDF page that carries no text comes back as an image. Each call needs a Tool call approval and fetches the page again, and what it returned is kept only in the Run's Context Journal, like any tool result. Aidash does not check that text a Run publishes cites only pages it read. The Data Protection Level does not govern it.
_Avoid_: Research (former category), Research operation, Web disclosure approval (former name of its Tool call approval), Document snapshot, Source citation, Evidence Revocation, web open

**URL intent**:
A person's request, made by pasting a Web address into a Thread, that the Run read that page. It is recorded with the person's input, apart from its text, so words that look like one are not one. It only asks for a Page reading; the call still needs its own Tool call approval.
_Avoid_: URL approval, open URL, pre-approval

**Eligible approver**:
A person the Tenant's policy currently lets decide a pending capability approval, such as a Tool call approval. When the Run's requester is one, only they decide it, where they watch the Run, and their request never counts as the decision. Otherwise every Eligible approver is offered it and the first decision counts. Who is eligible is judged again whenever the approval is shown or decided, and they need no access to the Thread it came from.
_Avoid_: Designated approver, approver (unqualified), reviewer, approval group

**Run grant**:
An approver's decision to admit a Run's later requests of the same kind to the same destination until it expires. For Page reading the destination is the page's host, so the grant covers later pages and parts on that host the approver has not seen, and it also ends at the Run's next user input. For a call of a Tenant's tool the destination is one operation of the tool, and new user input does not end it. For a Sampling approval the destination is one MCP server of an Agent Plugin, and new user input does not end it either.
_Avoid_: standing approval, allowlist, allow for this Run (the button, not the record)

### Completion

**Working Area**:
The kept files of one subject's Runs of one Agent in one workspace thread: what an Agent explicitly registered from its Scratch Space as worth keeping, never what can be regenerated, such as build output or installed dependencies. Follow-up Runs share it, one at a time, in the order they join its queue; a Run waiting its turn has not failed. Its revision counts registrations and says nothing about whether its content changed.
_Avoid_: workspace (a different thing), sandbox, area (unqualified)

**Scratch Space**:
A named, disposable place where Agents actually work: they install tools, download data and keep build output there across commands, Runs and conversations. Every Workspace uses one Scratch Space, chosen when it is created or made just for it, and never changes it; everyone allowed to use the space shares its content. It has its own settings, such as its capacity, how long it may go unused and which hosts it may reach. Nothing in it is kept as a File version. After going unused for its set period its content is wiped the next time it is used, and someone can clear the content at any time; the space itself disappears only when someone deletes it. Only what is registered into a Working Area outlives its content.
_Avoid_: shared workspace (a Workspace is a different thing), scratch (unqualified), temp folder, sandbox

### Collaboration

**Thread**:
The ordered exchange inside one Workspace where a person requests work, answers questions, decides approvals, receives Deliverables and asks for corrections. Tasks, Runs, approvals and File versions are referred to from it, never copied into it. Each message records its sender: a person, or the Agent whose Run wrote it, never the person who started that Run.
_Avoid_: conversation (starting one creates a Thread), chat, channel

**Thread target**:
The Agent or Node a person addresses when starting work in a Thread. Work given to a Node starts as an open Root Task.
_Avoid_: Cluster (a retired Registry kind), Coordinator (a retired Cluster role)

**Thread Attachment**:
A file a person or an Agent posts together with one message in a Thread. Every reader of the Thread sees that it is there; an Agent's model sees its content only after that Agent selects it, under its own Run's authority. It is neither a Working Area File nor a Reference Document.
_Avoid_: attachment (unqualified), upload, channel attachment

**Image Description**:
A text written for one image in a Thread Attachment, by the person who posted it or, always, by the Agent that posted it. Only its writer changes it. It helps find the image and stands in for the image for a model that cannot read images.
_Avoid_: alt text, caption, generated description

**Tenant Embedding Model**:
The exact Registry embedding version used by a Tenant's Thread indexes, Built-in Memory and Drive indexes. A new Tenant starts on the built-in default.
_Avoid_: per-Agent embedding role, Thread embedding setting

**Embedding Switch**:
A Tenant Administrator's confirmed change to the Tenant Embedding Model after seeing an estimate of the data to re-embed. Searches keep using the old model until the new indexes are ready. Reference Documents keep the embedding model pinned to their Agent version.
_Avoid_: reindex (same model), migration (schema)

**Deliverable**:
The Drive versions, Completion Message attachments and Outside Versions that a Task presents as its result.
_Avoid_: artifact, output, result (unqualified)

**Correction Request**:
A person's request in a Thread to change one Deliverable, naming the exact File versions it concerns. It starts a new Task that records what it corrects; the corrected File versions stay as they were, and nothing the earlier Task did outside Aidash is repeated or approved again without a fresh approval.
_Avoid_: revision (counts a Working Area's publications), redo, rerun, retry

**Schedule**:
A recurring trigger, a cron expression in a time zone or a fixed interval, that starts a Task for one Agent of its Tenant with a fixed request, each time as a Run whose delegation chain starts at its Registrant. A tick missed while Aidash was down runs once late, unless it is later than the Schedule allows; a tick that comes while the previous Run is unfinished is skipped. A person registers one directly, or an Agent registers one for the person at the root of its Run's chain; a Run started by a Schedule never registers or changes one. There are no Tenant-owned Schedules.
_Avoid_: cron job, routine, recurring task, trigger (unqualified)

**Registrant**:
The person a Schedule acts as. When the Registrant leaves the Tenant or loses access, the Schedule stops; it runs again only after another person takes it over, who then becomes its Registrant.
_Avoid_: owner, schedule creator

**Reference file**:
One document uploaded for Agents to read on demand, placed at a folder path by the Source that binds it. Its text reaches a model only when an Agent reads it, unlike a Reference Document.
_Avoid_: Reference Document (an agent's private knowledge, sent with every request), attachment (Thread Attachment)

**Reference set**:
All the Reference files one Agent version can read, across every Source it binds. Its limits count all of them together.
_Avoid_: document set, knowledge

**Reference copy**:
A Reference file copied into a Working Area. As made, only the file's uploader can download it, and it cannot be shared, not even copied again with the same content. Revoking the file makes the Area holding it unusable; withdrawing the Source's Catalog Approval does not. Once edited, it is the editor's own file.
_Avoid_: materialized file

**Source**:
A definition in the Registry that gives the Agent versions binding it something to draw on, such as Workspace retrieval, memory, Skills or Reference files. It is not a Source record, which identifies a candidate Web source.
_Avoid_: Reference Source, Reference Attachment Source

**Supplied key**:
Key Material for a Provider Credential that did not come from Provider Authorization: pasted by a person in the dashboard, or stored through Host Access. Only a Self-hosted Deployment accepts one, because its host already reads the Secret Store. Every Provider Credential records whether its current key was authorized, pasted or stored through Host Access.
_Avoid_: BYOK key, manual key, imported key

**Bootstrap**:
The Host Access step that gives a fresh Self-hosted Deployment its first Tenant and administrators: it names or creates a Tenant, reserves an email address as that Tenant's Tenant Administrator, and grants the address Deployment Administrator, effective at once or at its first sign-in. Running it again never makes a second Tenant. When the Tenant's original administrator subject is unusable, it makes the address a new Tenant Administrator instead of restoring that subject, and it grants Deployment Administrator even when the Tenant cannot be recovered.
_Avoid_: setup wizard, first-run admin, initial operator

**Stint**:
One Agent's unbroken stretch of work on one Task, from waking until it goes to sleep. It ends when the Task enters a wait the Agent cannot move forward itself, such as a person's answer, an outside result or a child Task, so one Task may span many Stints; input that arrives while it is awake and a takeover after a crash stay inside it.
_Avoid_: Run (its former name), session, shift, activation

**Duty**:
A standing responsibility in a Node that one Agent carries out for the person who set it up, with its mission, procedure, triggers, routing, budget, memory and its own Thread. It never completes and has no authority of its own: every Task it starts acts for its requester, and it stops when that person leaves until its named successor takes it over, becoming its requester while it stays stopped. Deleting its Thread stops it too; its requester's next edit or resume opens a new Thread for it, without the old Thread's memory. A Node may hold any number of Duties.
_Avoid_: post, role, standing task, routine, watcher (one of its parts)

**Root Task**:
The topmost Task of one request, which a person's request in a Thread or a Duty's trigger creates; the Tasks an Agent splits it into hang below it and use the Scratch Space it chose.
_Avoid_: Workspace, goal, project

**Tenant Instructions**:
Standing directions for every Agent of a Tenant, which only its Tenant Administrators change. They rank below human answers and above Node Instructions.
_Avoid_: system prompt, global prompt, company policy (the Tenant's policy decides authority, not instructions)

**Node Instructions**:
Standing directions for every Agent working in one Node, which only that Node's administrators change. They rank below Tenant Instructions and above the Agent definition's instructions.
_Avoid_: system prompt, department prompt, Node goal

**Duty Trigger**:
A condition that wakes a Duty: a time, a request arriving at its webhook, a Watcher's finding, an event from a Node, or the completion of another Duty's Task.
_Avoid_: Schedule (now one kind of Duty Trigger), cron job, hook

**Watcher**:
A script of a Duty that looks at an outside service on its own schedule and turns what it finds into events for the Duty.
_Avoid_: poller, monitor, Agent (a Watcher uses no model)

**Router**:
The script of a Duty that decides, for each event no Task has booked, whether to drop it, act on it itself, give it to an open Task or start a new Task; a Duty without one starts a new Task per event.
_Avoid_: dispatcher, filter, rule

**Budget**:
An optional spending allowance on a Tenant, Node, Duty or person, per month or in total, with an optional Soft Limit that mails chosen recipients and an optional Hard Limit that stops, or puts to sleep, the work it covers. What an Agent creates draws on its creator's Budget.
_Avoid_: quota, token budget, spend cap

**Drive**:
The versioned files one Node keeps as its own: what its Tasks published there when they completed and what people put there directly. Everyone in the Tenant may read it unless the Node closes it, only work inside that Node writes to it, and when the Node is deleted its Drive is moved to another Node or deleted with it.
_Avoid_: Cabinet, Library, Archive, shared drive, Google Drive (an outside tool a Deliverable may be sent to), Reference set (an Agent version's fixed reading material)

**Instruction precedence** (supersedes the earlier entry, #1010):
The order in which a model resolves conflicting directions: the Harness Contract, then human answers, then Tenant Instructions, then Node Instructions, then the Agent definition's instructions and Skills, then a Duty's mission and procedure, then the task and its messages, then Repository rules, then data, which never directs. It shapes only how work is done; runtime authorization alone decides what may execute.
_Avoid_: priority, override order, trust order

**Goal**:
The completion condition of one Task, supplied by its requester, its Duty's procedure or the Agent that created it. When the Agent stops with an answer, the Goal judge returns met, not yet met or impossible. A Task without a Goal completes when its Agent says it is done.
_Avoid_: Completion Condition, Duty Goal, Completion Policy, acceptance criteria

**Scratch Space** (supersedes the earlier entry, #1019):
A disposable place where Agents actually work: they install tools, download data and keep build output there across commands and Tasks. Every Node has one that its Threads, Duties and Subnodes share, and anyone who may give work to the Node can see it; a separate named one can be made in the Node for work that must be kept apart. Nothing in it is kept as a File version and nothing wipes it on a timer: its content goes only when someone clears it or its Node goes. What should outlive it is published to a Drive.
_Avoid_: shared workspace, scratch (unqualified), temp folder, sandbox, desk (an analogy only)

**Completion Message**:
The Thread message that records a Task's final answer, its verdict when it had a Goal, and its Deliverable. The Task posts it after a met or impossible verdict, or when its Agent finishes a Task that has no Goal.
_Avoid_: completion report, Report, artifact, result message

**Outside Version**:
One exact version of something kept outside Aidash, such as a commit, a document revision or a sent message, that a Deliverable names. Aidash records it without asking the outside system whether it exists.
_Avoid_: Outside Receipt, Delivery Note, external reference, link

**Subnode** (supersedes the earlier entry, #1012):
A group of agents that an agent creates inside its own Node to carry out part of a larger Task, lasting until its creating Agent deletes it from any of its Runs for the same person (`delete_subnode`), the creating Run is cancelled, or the creating Agent is disabled or removed; the creating Run's completion leaves it in place. Subnodes nest, and neither their number nor their members are limited. It is not a Node: it has no identity or address of its own, and its members may use only what the creating Run, the Subnode's allowance and their own definitions all permit.
_Avoid_: team (unqualified), sub-Node, child Node

**Schedule** (superseded, #1011):
No longer a concept of its own: a Duty with a time trigger replaces it. The Schedules of #557 became Duties; their Registrant is the Duty's requester, and the rule that a Run a Schedule started never registers one is gone (#1012).
_Avoid_: Schedule, Registrant (say Duty, time trigger, requester)

### Reports

**KGI**:
A business result a person sets for a Tenant or a Node, with a target. Its number comes only from Aidash's records, a Marketplace measurement, a certified script or a person's entry, never from a number a model wrote. A Tenant and each Node hold only as many as the Tenant's limit allows.
_Avoid_: Goal (a Task's completion condition), objective, OKR, metric (unqualified)

**KPI**:
A measure declared for one Duty, by the Agent that created it or by a person, naming the KGI it serves. It shows as uncertified until it is Certified, and a number the Agent itself wrote is shown as self-reported.
_Avoid_: metric (unqualified), key result, goal

**Certification**:
A permitted person's label on a KPI or a KGI's measurement saying it measures what it claims. It never blocks anything; changing how the number is taken removes it.
_Avoid_: approval (a Catalog Approval gates use; Certification only labels), endorsement, verification

**Measurement**:
How one KGI or KPI gets its number: Aidash's own records, a Marketplace measurement, a script, a person's entry, or the Agent's own statement (self-reported).
_Avoid_: data source, connector, integration

**Report**:
A summary Aidash builds for one person without a model, from the records that person may see, compared with the previous Report they received: their pending decisions, KGIs, KPIs, stopped work, each Duty's finished Tasks, Goal judgments, spend, and Duties that serve no KGI. It never lists tool calls.
_Avoid_: digest, dashboard, status report (a message an Agent writes)

**Atomic Transaction**:
A set of Thread, Task and Run changes on one or more Nodes that a coordinator Node commits on all of them or on none. The submitter's manifest names every change; only the coordinator holds it.
_Avoid_: distributed transaction (fine in prose), saga, batch

**Transaction Share** (#1107, ADR 0146):
What one participant of an Atomic Transaction holds: the transaction header, every participant's Seal and only its own changes. A participant never receives another participant's changes.
_Avoid_: manifest (the coordinator's whole list), slice

**Seal**:
What every participant learns about one participant's changes: a hash that binds them and one Claim per Task completion or Run finalization. All participants agree on the Seals through the transaction digest.
_Avoid_: commitment (fine in prose), fingerprint

**Claim**:
A hash naming one Task whose completion or Run finalization a participant's changes include. A participant checks a paired obligation against its counterpart's Claims without seeing its changes.
_Avoid_: obligation (the pairing rule itself), assertion
