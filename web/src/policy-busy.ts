import { useEffect, useState } from "react";
import { ApiError } from "./transport";

/**
 * Offers again the change a Policy Busy refusal left undone, once its
 * `Retry-After` has passed. It never sends by itself: each attempt can hold up
 * the Tenant's work.
 */
export function useBusyRetry() {
  const [offer, setOffer] = useState<{
    send: () => void;
    wait: number;
  } | null>(null);
  const [ready, setReady] = useState<typeof offer>(null);
  useEffect(() => {
    if (!offer) return;
    const timer = window.setTimeout(() => setReady(offer), offer.wait * 1000);
    return () => window.clearTimeout(timer);
  }, [offer]);
  return {
    /** Records how an attempt ended: `reason` is what it threw. */
    note(reason: unknown, send: () => void) {
      setOffer(
        reason instanceof ApiError && reason.retryAfter !== undefined
          ? { send, wait: reason.retryAfter }
          : null,
      );
    },
    clear() {
      setOffer(null);
    },
    /** The props that put the retry on an `Alert`. */
    alert: (offer
      ? { retry: offer.send, retryDisabled: ready !== offer }
      : {}) as { retry?: () => void; retryDisabled?: boolean },
  };
}
