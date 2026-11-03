# Stripe billing sandbox acceptance

Task 0 remains **blocked on an available Stripe Sandbox credential and actual
human Checkout**. The runner and offline tests do not establish Managed
Payments compatibility. No billing model, production integration, launch price,
Terraform change, or Issue acceptance follows from this document alone.

## Scope and current API contract

The runner uses Python 3.9+ standard-library HTTPS, pins `Stripe-Version` to
`2026-09-30.endive`, and contacts only `https://api.stripe.com/v1/`. Stripe lists
that version as current on the [versioning page](https://docs.stripe.com/api/versioning).
Managed Payments setup requires activation in the selected Sandbox and API
`2025-03-31.basil` or later; its Checkout request sets
`managed_payments[enabled]=true`. See the [official setup guide](https://docs.stripe.com/payments/managed-payments/set-up).

The disposable Product uses `txcd_10103001` (business SaaS), listed among
[eligible Managed Payments tax codes](https://docs.stripe.com/payments/managed-payments/eligibility).
It represents an automated software service, not inference resale. This test
classification does not replace the Stripe account holder's actual product eligibility review.
The fixture price is USD 1 per unit, tax inclusive; seven units should produce a
USD 7 Sandbox charge. These amounts are test fixtures, not commercial pricing.

The experiment deliberately tests Billing Meters as requested in Issue #242.
Stripe's current [usage recording guide](https://docs.stripe.com/billing/subscriptions/usage-based/recording-usage)
now recommends Metronome for new integrations. That recommendation does not
prove either compatibility or incompatibility of this specific experiment.

## Credential and result handling

A Stripe account holder supplies an explicit JSON file, normally under the ignored
`secrets/` directory, containing exactly one `STRIPE_SECRET_KEY` property with
the Sandbox secret key. Set its permissions to `0600`. Do not put the key on the
command line, source an environment file, or paste it into an Issue. The runner
accepts only `sk_test_...` or `rk_test_...`; live and publishable keys are refused
before a network request. Restricted keys need permission for the resources
listed below. The key prefix alone does not distinguish legacy test mode from
a separate Sandbox: the account holder must confirm the selected Dashboard Sandbox.

No credentials are currently available for this task. Do not search existing
environment or secret files to obtain them. The no-credential invocation is safe:

```sh
python3 scripts/stripe-sandbox-spike.py --output notes/stripe-spike.json
```

It exits `2`, emits `winner: inconclusive` and `reason: credential_required`, and
makes no network request. This is readiness evidence only.

After an account holder supplies credentials, use the same output path for every
phase. Each command prints only a validated JSON projection: run ID, fixed API
version, Stripe request IDs, object IDs, normalized compatibility error, numeric
billing evidence, booleans, and fixed reason values. No HTTP response body,
error text, headers, Checkout URL, customer contact information, card details,
or key is logged. Result files are written atomically with `0600` permissions
under `notes/`. Keep the run's result until cleanup and acceptance are recorded.
The file is a resume journal for a trusted account holder; it is not a signed receipt.
Do not edit evidence or copy IDs from unrelated resources into it.

HTTPS redirects and environment-configured proxies are disabled. HTTP timeout
is 30 seconds. There is no automatic retry or unbounded polling. Reissue the
same phase when pending. POSTs use a stable run/step idempotency key, and usage
also has a stable event identifier. Writes are refused after 23 hours because
Stripe can prune [idempotency keys](https://docs.stripe.com/api/idempotent_requests)
after 24 hours. For an old uncertain write, inspect its request ID in Workbench;
do not discard the journal and blindly recreate it. A journal explicitly marked
as never attempted may start its write window when credentials become available.
A durable `write_attempted` marker is saved before the first POST is sent.
Even if that POST times out before returning any request/object ID, its run ID,
idempotency key and original write window survive retries and no-credential
invocations. Earlier journals without the marker are conservatively treated as
possibly attempted, retaining their original window. All persisted ID slots
require string scalars; expanded API objects are never retained in those slots.
Run only one process against a journal at a time.

## Metered experiment and human checkpoints

Run commands from the checkout root. These commands are intentionally separate;
there is no API shortcut that claims to complete a hosted customer Checkout.
The runner never creates a subscription outside Checkout, manually creates an
invoice, or confirms an unrelated PaymentIntent to manufacture acceptance.
[Managed Payments restricts those integration paths](https://docs.stripe.com/payments/managed-payments).

1. Create the disposable Product, Test Clock, clock-associated Customer, sum
   Billing Meter, monthly metered Price, and Managed Payments Checkout Session:

   ```sh
   python3 scripts/stripe-sandbox-spike.py --credential-file secrets/stripe-sandbox.json --output notes/stripe-spike.json --phase prepare
   ```

   This exits `2` with `checkout_pending` even if Session creation succeeds.
   The metered line omits `quantity`. Objects carry `aidash_spike=<run_id>`
   metadata where supported; the meter event name and clock name contain the
   run ID. Inspect the retained request IDs in Stripe Workbench if any step
   fails. No fallback objects are created automatically.

2. Open the hosted Session without logging its URL:

   ```sh
   python3 scripts/stripe-sandbox-spike.py --credential-file secrets/stripe-sandbox.json --output notes/stripe-spike.json --phase checkout --open-checkout
   ```

   The account holder completes the Stripe-hosted page using [Stripe test payment
   details](https://docs.stripe.com/testing#cards) and fictional buyer contact
   information. The configured redirect is `https://localhost:4242/success`;
   a local success page is optional and its availability is not proof of payment.
   Rerun `--phase checkout` to check API completion. The Session must be
   `complete`, `livemode=false`, owned by this run, and have Managed Payments
   enabled. Completion by itself still leaves `winner: inconclusive`.

3. Record exactly seven units and wait for asynchronous aggregation:

   ```sh
   python3 scripts/stripe-sandbox-spike.py --credential-file secrets/stripe-sandbox.json --output notes/stripe-spike.json --phase usage
   python3 scripts/stripe-sandbox-spike.py --credential-file secrets/stripe-sandbox.json --output notes/stripe-spike.json --phase aggregate
   ```

   Usage requires the completed Checkout's active subscription, matching Price,
   Customer and clock. The runner timestamps one event at the current clock
   time within the original service period. See [meter creation](https://docs.stripe.com/api/billing/meter/create),
   [event creation](https://docs.stripe.com/api/billing/meter-event/create), and
   [event summary parameters](https://docs.stripe.com/api/billing/meter-event-summary/list).
   Reissue `aggregate` until `usage_aggregated=true` and
   `aggregated_quantity=7`; an accepted event alone is insufficient. Summary
   bounds align to minutes. Do not advance the clock before aggregation.

4. Advance the original subscription period through a supported Test Clock,
   then allow finalization and payment processing:

   ```sh
   python3 scripts/stripe-sandbox-spike.py --credential-file secrets/stripe-sandbox.json --output notes/stripe-spike.json --phase advance
   ```

   Reissue `advance` after the clock finishes until `cycle_advanced=true`.
   Then run:

   ```sh
   python3 scripts/stripe-sandbox-spike.py --credential-file secrets/stripe-sandbox.json --output notes/stripe-spike.json --phase settle
   ```

   Reissue `settle` until `settlement_advanced=true`. The first advance targets
   the saved original service-period end; settlement targets two hours later.
   Stripe documents asynchronous clock advancement and a default one-hour
   invoice finalization delay in [API simulation guidance](https://docs.stripe.com/billing/testing/test-clocks/api-advanced-usage).
   Custom grace periods, asynchronous payment methods, account restrictions,
   or Managed Payments/Test Clock limitations may prevent this from settling.
   Such a result stays inconclusive. Do not replace it with a manually paid
   invoice or interpret a Test Clock refusal as metered incompatibility.

5. Verify the complete billing chain:

   ```sh
   python3 scripts/stripe-sandbox-spike.py --credential-file secrets/stripe-sandbox.json --output notes/stripe-spike.json --phase verify
   ```

   Only `verify` can return `winner: metered_overage` and exit `0`. It rereads
   Checkout, the active subscription, aggregated usage and ready clock. It
   requires a `subscription_cycle` invoice linked to this Customer, subscription
   and Test Clock, exactly one line for the tested Price and original period,
   quantity seven, amount USD 7, and invoice paid in full. It follows a paid
   [Invoice Payment](https://docs.stripe.com/api/invoice-payment/list) to its
   succeeded PaymentIntent and [Charge](https://docs.stripe.com/api/charges/object),
   requiring the same USD 7 to be paid, captured and unrefunded. A zero initial
   invoice, account credit, an open invoice, wrong quantity, or pending charge
   cannot establish acceptance. Record the final safe JSON plus verification date
   and Sandbox context in Issue #242 and the local Task 0 report.

If Stripe does not support using this clock-associated Customer with Managed
Payments Checkout, the harness cannot prove the metered charge in this run.
Keep `inconclusive`, record request IDs, and obtain a supported simulation path
from Stripe before changing the harness. Documentation for the separate
features is not empirical proof that their combination works.

## Conditional Usage Credits experiment

Fallback is permitted only when metered Session creation returns HTTP 400,
`invalid_request_error`, and an explicit statement that Managed Payments does
not support metered prices/billing/subscriptions or usage-based billing. The
runner normalizes that result to
`compatibility_error.kind=metered_managed_payments_unsupported` and retains its
request ID. It deliberately recognizes narrow wording; unfamiliar refusals
remain inconclusive for investigation. Authentication, permissions, invalid
parameters, missing activation, product eligibility, Test Clock constraints,
network errors, rate limits and server failures do not authorize fallback.

After that definitive refusal only:

```sh
python3 scripts/stripe-sandbox-spike.py --credential-file secrets/stripe-sandbox.json --output notes/stripe-spike.json --phase fallback
python3 scripts/stripe-sandbox-spike.py --credential-file secrets/stripe-sandbox.json --output notes/stripe-spike.json --phase checkout --open-checkout
```

`fallback` creates an owned one-off USD 7 tax-inclusive Price and Managed
Payments `mode=payment` Checkout. Complete that hosted page with test details,
then run `--phase verify`. It requires the matching single-price line, completed
and paid Session, and succeeded, captured, unrefunded USD 7 charge before it can
emit `winner: usage_credits`. This proves only the one-off purchase transport;
production credit grants, ledger correctness and consumption still need their
own implementation and verification. Never choose this model because the
metered run is slow, unavailable, unauthorized, or incomplete.

## Cleanup and delivery evidence

The runner makes no automatic deletions. Retain the result first. In the selected
Sandbox Dashboard, match every resource to the exact journal ID and run marker
before acting. Expire only this run's open Sessions, cancel only its disposable
subscription, and archive only its Prices/Product and deactivate its meter when
no longer used. Delete this run's Test Clock only after recording billing proof;
[deleting a simulation](https://docs.stripe.com/billing/testing/test-clocks/api-advanced-usage#delete-clock)
also removes associated customers and subscriptions. Keep any payment/invoice
history Stripe retains. Do not delete or archive resources merely because
names look similar, and do not remove shared/account-level settings. An
uncertain timeout can leave an owned resource not yet present in the journal;
reconcile its stable request/idempotency identity in Workbench first.

Acceptance requires the actual final Sandbox JSON and corresponding selected
implementation. The following remain separate claims: offline test success,
Session acceptance, human Checkout completion, metering aggregation, renewal
quantity, charge settlement, production implementation, and live acceptance.
At present only offline harness readiness is available. Do not close Issue #242
or Task 0 on that basis.

Offline checks:

```sh
python3 -m unittest discover -s scripts -p test_stripe_sandbox_spike.py -v
python3 -m py_compile scripts/stripe-sandbox-spike.py scripts/test_stripe_sandbox_spike.py
```
