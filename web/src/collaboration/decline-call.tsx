import { useRef, useState } from "react";
import { Button } from "../components/ui/button";
import { Textarea } from "../components/ui/textarea";
import { Hint, formClass } from "../components/patterns";
import { Field, useI18n } from "../ui";
import type { Submit } from "../forms";
import {
  declineKindKey,
  nextDeclineStep,
  type DeclinableCall,
} from "./decline-call-model";

/**
 * Decline the call a Run is paused on, so the Run goes on without it (#763). The optional
 * sentence is an ordinary Run message. By default it goes first; if declining then fails, the
 * message stays in the Thread, the Run stays paused and the error is shown by `submit`. For a
 * Run on another Node (`order="decline-first"`) the receiver takes no message while the Run is
 * paused, so the call is declined first. Someone who cannot read the Run cannot message it, so
 * `withSentence={false}` offers the button alone.
 */
export function DeclineCall({
  call,
  submit,
  send,
  decline,
  order = "sentence-first",
  withSentence = true,
  onDeclined,
  onFinished,
}: {
  call: DeclinableCall;
  submit: Submit;
  /** Send the sentence as a Run message. */
  send?: (content: string, key: string) => Promise<unknown>;
  decline: (call: string, restOfRun: boolean) => Promise<unknown>;
  order?: "sentence-first" | "decline-first";
  withSentence?: boolean;
  /** The call is declined, though the sentence may still be unsent. The Run goes on, so a parent
   *  that shows this form only while the call is offered must keep showing it. */
  onDeclined?: () => void;
  /** Every step went through and the form closed. */
  onFinished?: () => void;
}) {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);
  const [sentence, setSentence] = useState("");
  const [restOfRun, setRestOfRun] = useState(false);
  const [sent, setSent] = useState(false);
  const sentRef = useRef(false);
  const [declined, setDeclined] = useState(false);
  const declinedRef = useRef(false);
  const [key] = useState(() => crypto.randomUUID());
  const [busy, setBusy] = useState(false);
  if (!open)
    return (
      <Button variant="outline" type="button" onClick={() => setOpen(true)}>
        {t("declineCall")}
      </Button>
    );
  const what = t("declineCallWhat")
    .replace("{tool}", call.tool)
    .replace("{refuser}", t(declineKindKey(call.kind)));
  async function confirm() {
    setBusy(true);
    try {
      // One submission: a successful one closes the Run's dialog, so the message and the decline
      // must not be two. The refs record each step that went through at once, so a retry repeats
      // only the step that has not, never a second decline or a second message.
      const ok = await submit(async () => {
        for (;;) {
          const step = nextDeclineStep({
            sentence: withSentence && send ? sentence : "",
            sent: sentRef.current,
            declined: declinedRef.current,
            order,
          });
          if (step === "done") return;
          if (step === "message") {
            await send!(sentence.trim(), key);
            sentRef.current = true;
            setSent(true);
          } else {
            await decline(call.call, restOfRun);
            declinedRef.current = true;
            setDeclined(true);
            onDeclined?.();
          }
        }
      });
      if (ok) {
        setOpen(false);
        setSentence("");
        setRestOfRun(false);
        sentRef.current = false;
        declinedRef.current = false;
        setSent(false);
        setDeclined(false);
        onFinished?.();
      }
    } finally {
      setBusy(false);
    }
  }
  return (
    <form
      className={`${formClass} min-w-0 basis-full`}
      aria-label={t("declineCallHeading")}
      onSubmit={(event) => {
        event.preventDefault();
        void confirm();
      }}
    >
      <p className="text-[13px] font-semibold text-foreground">
        {t("declineCallHeading")}
      </p>
      <p className="text-xs text-foreground [overflow-wrap:anywhere]">{what}</p>
      <Hint>{t("declineCallExplain")}</Hint>
      <label className="flex items-center gap-2 text-xs">
        <input
          type="checkbox"
          checked={restOfRun}
          disabled={busy || declined}
          onChange={(event) => setRestOfRun(event.target.checked)}
        />
        {t("declineCallRestOfRun")}
      </label>
      {withSentence && send && (
        <>
          <Field label={t("declineCallSentence")}>
            <Textarea
              maxLength={16000}
              value={sentence}
              disabled={sent || busy}
              onChange={(event) => setSentence(event.target.value)}
            />
          </Field>
          <Hint>
            {t(
              order === "decline-first"
                ? "declineCallSentenceAfterHint"
                : "declineCallSentenceHint",
            )}
          </Hint>
        </>
      )}
      {declined && !sent && sentence.trim() && (
        <Hint role="status">{t("declineCallAlreadyDeclined")}</Hint>
      )}
      <div className="flex min-w-0 flex-wrap gap-2">
        <Button variant="destructive" type="submit" disabled={busy}>
          {t("declineCallConfirm")}
        </Button>
        <Button
          variant="ghost"
          type="button"
          disabled={busy}
          onClick={() => setOpen(false)}
        >
          {t("cancel")}
        </Button>
      </div>
    </form>
  );
}
