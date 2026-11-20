# Duties

A **Duty** is a standing responsibility a Node carries out for a person
([ADR 0142](../adr/0142-a-duty-acts-only-for-its-requester-and-an-event-is-input-never-a-requester.md),
[#1011](https://github.com/kent8192/aidash/issues/1011)). It has a mission, a
procedure, Duty Triggers and its own Thread, and one Agent carries it out. A
Duty absorbs the Schedules of #557: a time trigger is one kind of Duty Trigger.

A Duty has no authority of its own. It always acts as one person, its
**Requester**: every Run it starts has the Requester at the root of its
delegation chain, so their Approval Mode, Approval Rules, Plan Connections and
MCP OAuth Connections apply exactly as for a Run they start by hand (ADR 0134).
A Duty whose Requester leaves stops until a named successor takes it over.

## Setting one up

A person sets up a Duty in a Thread's **Duties** tab, or through
`POST /api/threads/{thread}/duties`. It needs `thread.read`,
`duty.create` and `task.create` on the Thread; members hold all three by
default.

```json
{
  "title": "Daily report",
  "mission": "Summarize yesterday's merged pull requests.",
  "procedure": "1. Read every merged pull request.\n2. Group them by area.",
  "agent": { "id": "reporter", "version": "1.0.0" },
  "trigger": {
    "kind": "cron",
    "expression": "0 9 * * 1-5",
    "time_zone": "Asia/Tokyo"
  },
  "approval_mode": "dont_ask",
  "max_lateness_seconds": 3600,
  "pause_after_failures": 3
}
```

- `mission` is what every Task the Duty starts is asked. `procedure` is the
  checklist its Agent follows on every Task.
- `trigger` is the time trigger, if the Duty has one: a five-field cron
  expression (minute, hour, day of month, month, day of week, where Sunday is
  0 or 7) in an IANA time zone, or `{"kind":"interval","every_seconds":N}`
  counted from registration or resume. There is no minimum interval and no
  count limit (#1012): a Budget, not a count, bounds what a Duty spends. On
  Cloud a Tier may cap how many Duties a Tenant has. A small hidden jitter
  delays each tick by up to 30 seconds. A time a daylight-saving change skips
  runs once at the first instant after the gap; a repeated time runs once.
  An edit removes the time trigger with `{"kind":"none"}`.
- `approval_mode` is the mode of the Duty's Runs, within what the Tenant's
  policy lets the Requester use. Without it, their own default applies.
- `pause_after_failures` is how many failed Tasks in a row pause the Duty: 3
  by default, 0 never pauses it.
- The Agent version is pinned. The Duty's page shows when a newer version
  exists; the Requester moves it with an edit.

The Requester edits, pauses, resumes, deletes or runs a Duty now from the
Duties tab or **Settings → Duties** ("my Duties"). "Run now" starts an
ordinary Run as the person who clicked it, in their own Approval Mode, and
records it in the history. Deleting a Duty removes the Budgets that target
it, so deleting one whose Budget someone else set needs `budget.manage`.
Deleting it also cancels its Router and Watcher Tasks that have not finished.

A Duty runs on its Requester's Mapping credential, whichever credential
set it up, so an API token may expire without stopping it; disabling the
Mapping stops it.

## The Duty's Thread

Every Task of a Duty works in the Duty's own Thread (#1008, ADR 0140).
Registering the Duty opens that Thread, in the Node and Scratch Space of the
Thread it was registered from and created by its Requester, with the title
and mission as its first message; the Duty records both Threads
(`thread_id`, `origin_thread_id`), so the Agent tools of the Thread it came
from still list and change it. The Requester talks to the Duty there, and its
Tasks report there. Deleting the Thread stops the Duty.

## Other Duty Triggers

Besides its time trigger, a Duty wakes on the triggers its Requester adds with
`POST /api/duties/{id}/triggers`:

- `{"kind":"webhook"}` returns a URL under `/hooks/duties/{trigger}` and its
  signing secret, shown only in that response. A request is accepted only when
  `X-Hub-Signature-256` (as GitHub sends it) or `X-Aidash-Signature` carries
  `sha256=<hex HMAC-SHA256 of the body>`; an unknown trigger and a wrong
  signature are refused alike with 404. The body, at most 256 KiB of JSON or
  UTF-8 text, is the event. The secret is derived from the Deployment's API
  token, so replacing the token replaces every webhook secret. A trigger
  takes each delivery once: for seven days the same body, or the same
  `X-GitHub-Delivery`, `X-Aidash-Delivery` or `X-Request-Id`, is answered 200
  with the first delivery's receipt and starts nothing. The delivery id is
  not signed, so the body decides: a sender whose events can repeat their
  body word for word must vary it, for example with an id or a timestamp. An
  event that could not be taken for now (a database outage, a policy change
  racing the start) is answered 503 and recorded nowhere, so the sender can
  send it again.
- `{"kind":"duty_completed","source_duty_id":…}` wakes the Duty whenever a
  Task of that other Duty ends; the event names the Duty, the Task and its
  status.
- `{"kind":"node_event"}` takes events another Node sends (#1013). The
  response names the Duty's Node (`node_id`); the Requester gives that and
  the trigger's id to whoever should send. Every sender needs the Tenant's
  trust in its Node: within the Tenant, the policy lets its subject use the
  Duty's Node (`node.use`); from another Tenant, this Tenant's administrator
  registered the Node (and a Peer Mapping when it sends as a named subject).
  `"from_node":"aidash://…"` admits that Node alone and takes its events.
  An open trigger takes only a sender of the same Tenant whose subject may
  read the Duty, and never another Tenant's Node: name the partner's Node
  with `from_node` to take its events. The event is recorded as
  `{"from_node": <the sending Node>, "data": <what it sent>}`, at most 256 KiB;
  the sending Node is the one its Federation token proved, never a field of
  the body. An unknown trigger, one of another Node and one that names another
  sender are refused alike with 404.

An event is input, never a requester (ADR 0142), whoever sends it: another
Tenant's event cannot command the Agent, change the Requester or grant
authority. It goes to the Task that booked it, if one did; otherwise to the
Duty's Router, if it has one (below). Without a Router it starts a new Task in
the Duty's Thread whose description is the mission followed by the event inside
`<event trust="data">`, marked as data that never directs the Agent, and while
the Duty's previous Task is open an event is skipped (`overlap`).
`GET /api/duties/{id}/events` lists what each event did: `started`,
`delivered`, `skipped` or `failed` with the reason, and with a Router
`routing`, `dropped` or `acted`. Events are data of the Duty's Thread and go
with the Duty.

## Router and Watcher scripts

A Duty may carry two scripts, deterministic Python or shell that runs without
a model, so cheap filtering never wakes one (#1011):

```json
{
  "router": { "language": "python", "source": "import json, sys\n..." },
  "watcher": {
    "script": { "language": "shell", "source": "curl -s …" },
    "every_seconds": 300
  }
}
```

A script's source is at most 64 KiB without NUL. The create body takes both;
an edit replaces one or removes it with `{"kind":"none"}`. Every saved version
is kept with its author and time and the Duty's page lists them. A script
needs no approval of its own (ADR 0142): it runs only when `code_interpreter`
(Python) or `shell` is approved in the Catalog for the Duty's Node and bound
to the Duty's Agent; otherwise its Task fails with that reason and the event
records it.

A script runs as a Task of the Duty's Agent for its Requester, in the Duty's
Thread and its Execution Pod, so Egress, External services, cancel and purge
apply as for any Run, but no model is asked: the harness writes the script and
its stdin into the Scratch Space and runs it through the bound tool, and the
script's stdout is the Task's result. Script Tasks are not ticks: they never
count as an overlap, toward `pause_after_failures` or for `duty_completed`
triggers.

- **Router.** Every event no Task booked goes to the Router instead of the
  default, and is recorded as `routing` until it decides. It reads on stdin
  `{"event": …, "trigger": {"id", "kind"}, "open_tasks": [{"task_id", "title"}]}`
  and prints one JSON object:
  - `{"action":"drop"}`: the event is `dropped`;
  - `{"action":"act"}`: the script did the work itself, `acted`;
  - `{"action":"task","title":"…","description":"…"}`: a model Task starts in
    the Duty's Thread, as an event's Task does. It keeps the Duty's own title:
    the Router's title and description reach the Agent only inside the
    event's data, since a Router often copies event text into them;
  - `{"action":"deliver","task_id":"…","input":"…"}`: the event goes to that
    open Task of the Duty as a booked event does, with the Router's `input`
    beside its payload, and wakes its Run.

  Anything else fails the event with a short reason. The event's `router`
  field records the decision.

- **Watcher.** Every `every_seconds` (one minute to one year), independent of
  the time trigger, the Watcher runs; it is skipped while its previous run is
  open. It prints a JSON array of event objects, each delivered as a webhook's
  is, with trigger kind `watcher` (to the Router if there is one). A run keeps
  32 events of at most 256 KiB each; the rest are dropped, with one `dropped`
  event saying how many.

A Run books the next event of a trigger of its root person's Duty by calling
`task_wait` with `on_trigger`. It sleeps until the event comes, for up to
30 days, and the event then wakes it: the wait's result carries the event as
`{"kind":"duty_event", …, "payload": …}`, data like every tool result (and the
Router's `input` when a Router delivered it). A Run
that arrived through Federation books none.

## Duty templates

A **Duty template** is a ready-made Duty in the Marketplace, such as a GitHub
CI duty or Gmail triage (#1011 D4). It is a package of kind `duty_template`,
browsed, read and shared like any other package (see
[Tenant Marketplace operations](../operations/marketplace-authorization.md#duty-templates)).
A template carries:

- the title, mission and procedure of the Duty it becomes;
- its Router and Watcher, if it has them, with the same shape and limits as a
  Duty's;
- the Duty Triggers it expects, each with a short instruction: a time trigger
  (`cron` or `interval`), `webhook`, `duty_completed` and `node_event`, at
  most one of each;
- the Agent it suggests, by id and version, if its publisher chose to.

It never carries a secret, a trigger secret, a Requester, a credential or an
id of the Tenant it came from. Every part of it refuses fields it does not
know, so one that names a secret is refused rather than silently stripped,
and its text, scripts and instructions are refused when they hold something
that looks like a credential: an Aidash Subject Credential, a GitHub, GitLab,
Slack, Anthropic, OpenRouter, OpenAI, AWS or Google API key, or a private key
block. A Watcher reaches an outside service through the Scratch Space's
External services, whose proxy adds the credential (ADR 0137), so a script
never needs one written in it. The same checks run when a template is
published and when it is installed, whichever Tenant published it.

**Installing** a template from a Thread,
`POST /api/marketplace/packages/{key}/duty` with the reviewed `digest`, the
`thread_id` and an `idempotency_key`, sets up a Duty whose Requester is the
person who installs it, as for any Duty they set up from that Thread: the Duty
opens its own Thread beside it, in the same Node and Scratch Space. It needs
`marketplace.read` and `marketplace.install` on the package and
`thread.read`, `duty.create` and `task.create` on the Thread. The
person picks the Agent (`agent`); without one, the suggested Agent staffs it,
which the Node must have approved like any Agent a Duty names. The Duty gets
the template's mission, procedure, scripts and time trigger, the Requester's
own default Approval Mode, the default lateness and a pause after 3 failures.
Its scripts need no approval of their own (ADR 0142): shell or Python must be
approved for the Node, as for any Router or Watcher. The template's other
triggers come back as `triggers_to_create`, each with its instruction, for the
person to create with `POST /api/duties/{id}/triggers`; a webhook's secret is
shown only then, to them. A retried install with the same key returns the
same Duty. A template is never installed as a Registry copy.

**Publishing** one of their Duties, `POST /api/duties/{id}/template`, is the
Requester's alone and needs `marketplace.publish`. The template is the Duty's
title, mission, procedure, scripts and time trigger, the kinds of its other
triggers with the instruction the publisher writes for each, and its Agent
when `suggest_agent` is true (the default). `GET /api/duties/{id}/template`
shows it first, with empty instructions. The body names the package
(`package_id`), a semantic `version`, the `author` and an optional
`description`; a version is immutable, and the new one is shared with the
publisher's Tenant until they share it further.

**Updates.** A Duty installed from a template records the template version
(`template`: key, Node, owner Tenant, package and version). When a later
version of the same package of the same publisher is readable and installable
by the viewer, the Duty's page offers it as `template_update`; nothing applies
it by itself. Its Requester applies it with
`POST /api/duties/{id}/template-update` (`key`, `digest`): the template's
mission, procedure and scripts replace the Duty's, and its title, Agent and
triggers stay as the Requester set them.

## Ticks

Every replica looks for Duties whose time trigger is due every
`[duties] poll_seconds` (15 by default). It claims due Duties with
`FOR UPDATE SKIP LOCKED` in the Requester's own transaction, starts the Task,
records the tick and moves the next time on, all in one transaction; a
scheduled time starts at most one Task.

| Tick                                                       | Recorded as                                                                            |
| ---------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| The Task started                                           | `started`, then `completed`, `failed` or `cancelled` with its Task                     |
| The previous tick's Task is unfinished                     | `skipped (overlap)`; a Router decides only about events, not ticks                     |
| Aidash was down past the maximum lateness                  | `skipped (late)`; earlier missed ticks fold into the latest one, which runs once, late |
| The usage gate refused the Run                             | `skipped (limit: <code>)`                                                              |
| Any other refusal, such as `COMPACTION_DECIDER_UNAPPROVED` | `failed` with the reason; never retried silently                                       |
| A transient error, such as a database outage               | nothing; the next pass tries the tick again                                            |

After `pause_after_failures` failed Tasks in a row the Duty pauses itself. It
stops when its Requester leaves the Tenant or loses access, when its Agent
version can no longer be started, when its Thread is deleted and when the
Tenant is deleted (a restore leaves it stopped). Deleting the Thread
deletes its Duties. The bell shows a **Duty stopped** notice to the Requester,
and to holders of `duty.administer` for a Duty waiting for a successor. A
Duty's Task completion appears in the bell without a toast; a failure also
toasts.

Only the Requester resumes a Duty. A holder of `duty.administer` may pause and
delete anyone's Duties and name a successor for one whose Requester left; it
runs again only when the successor resumes it, and they become its Requester.
The Duty's own Thread becomes theirs too, and the departed Requester's Runs
lose the trigger events they booked.

## Agents

The core capability `duties` (`core.duties@1`) is on by default; an Agent
definition removes its tools with `remove_default`. `duty_create`,
`duty_update` and `duty_delete` (which also pauses; it deletes no Duty a
Budget targets, since Agents never remove a Budget) are Tool call approvals of
the person at the root of the Run's chain, decided by their Approval Mode and
Approval Rules; `duty_list` needs none. A Duty an Agent sets up acts as that
person and belongs to the Run's Thread. A Run a Duty started may set up
Duties too (#1012): only the root person's authority and Budget bound what a
chain creates. A Run that arrived through Federation and a Run whose chain
starts at no person cannot set up or change Duties. A retried tool call
returns the Duty its first call set up.

A Duty an Agent sets up records the Task whose Run set it up, so the
[org chart](org-chart.md) shows the flow that made it, and stopping that flow
pauses it; only its Requester resumes it. A Duty may be staffed with a
generated agent only by the person whose request generated it, and the agent
stays live while such a Duty names it (see [generation](../generation.md)).

`duty_notify` sends an event to a `node_event` trigger on another Node, given
the Node and the trigger's id (#1013). It is a Tool call approval of the
person at the root of the Run's chain, like `duty_create`. It sends from the
Run's Node: to a Node of the same Tenant as that person, whom the Tenant's
policy judges there, and to a Node of another Tenant as the Node, which that
Tenant must have registered. A Run that arrived through Federation sends
none.

## Duties and other Nodes

A Duty never runs on another Tenant's Node (ADR 0142). A partner that should
do standing work for us sets up its own Duty with a `node_event` trigger, and
our Agents send it events with `duty_notify`; the partner's Duty acts only
for its own Requester.

A Task a Duty starts may hand work to other Nodes through Remote Grants like
any other Task, and a Run on another Node that a Remote Grant started may hand
it on again (#654): every hop is granted from the Task's Home Node. While the
Task is open, its Remote Grants stay live however long their Runs sleep, for
example on an approval, CI or a child Task; they end with the Task (see
[Remote Grants that sleep and hand work on](../protocol.md#remote-grants-that-sleep-and-hand-work-on)).
