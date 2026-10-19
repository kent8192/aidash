# The payment processor sends billing email, and Aidash sends none

The 2026-10-12 billing design had Aidash email the Billing Contact about failed payments, which
needed a new outbound email capability. Under Stripe Managed Payments, Stripe is the merchant of
record. It collects the customer's email in Checkout and sends receipts, failed-payment notices
and expiring-card notices itself, through Link. Aidash therefore builds no email capability: the
Billing Contact is the address Stripe collected, and Aidash reports the dunning grace period and
any suspension only in the dashboard.

## Consequences

- An Aidash notice that Stripe does not send, such as a suspension that a Tier limit caused, is
  shown only in the dashboard until some later Issue adds email.
- In a Stripe sandbox, receipts are not emailed automatically, so tests cannot rely on them.
