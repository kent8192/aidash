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
 * sentence is an ordinary Run message sent first; if declining then fails, the message stays in
 * the Thread, the Run stays paused and the error is shown by `submit`.
 */
export function DeclineCall({
  call,
  submit,
  send,
  decline,
}: {
  call: DeclinableCall;
  submit: Submit;
  /** Send the sentence as a Run message. */
  send: (content: string, key: string) => Promise<unknown>;
  decline: (call: string) => Promise<unknown>;
}) {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);
  const [sentence, setSentence] = useState("");
  const [sent, setSent] = useState(false);
  const sentRef = useRef(false);
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
      // must not be two. The ref records a sent message at once, so a retry never sends it again.
      const ok = await submit(async () => {
        if (
          nextDeclineStep({ sentence, sent: sentRef.current }) === "message"
        ) {
          await send(sentence.trim(), key);
          sentRef.current = true;
          setSent(true);
        }
        return decline(call.call);
      });
      if (ok) {
        setOpen(false);
        setSentence("");
        sentRef.current = false;
        setSent(false);
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
      <Field label={t("declineCallSentence")}>
        <Textarea
          maxLength={16000}
          value={sentence}
          disabled={sent || busy}
          onChange={(event) => setSentence(event.target.value)}
        />
      </Field>
      <Hint>{t("declineCallSentenceHint")}</Hint>
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
