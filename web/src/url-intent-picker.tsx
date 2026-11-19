import { Hint } from "./components/patterns";
import { Button } from "./components/ui/button";
import { useI18n } from "./ui";
import { MAX_URL_INTENTS } from "./url-intents";

/**
 * One chip per readable URL in the input. Selecting one asks the Run to read
 * that page; the page reading it leads to still needs its own Tool call
 * approval.
 */
export function UrlIntentPicker({
  candidates,
  selected,
  onChange,
}: {
  candidates: string[];
  selected: string[];
  onChange: (pages: string[]) => void;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  if (candidates.length === 0) return null;
  return (
    <div
      role="group"
      aria-label={ja ? "このページを読む" : "Read this page"}
      className="flex flex-wrap items-center gap-1.5"
    >
      {candidates.map((url) => {
        const on = selected.includes(url);
        return (
          <Button
            key={url}
            type="button"
            size="sm"
            variant={on ? "default" : "outline"}
            aria-pressed={on}
            disabled={!on && selected.length >= MAX_URL_INTENTS}
            onClick={() =>
              onChange(
                on
                  ? selected.filter((page) => page !== url)
                  : [...selected, url],
              )
            }
          >
            {ja ? "このページを読む" : "Read this page"}:{" "}
            <span className="max-w-[24ch] truncate font-mono">{url}</span>
          </Button>
        );
      })}
      <Hint>
        {ja
          ? "依頼しても読み取りは承認されません。各ページの読み取りには Tool call の承認が必要です。"
          : "Asking does not approve reading: each page reading still needs its own Tool call approval."}
      </Hint>
    </div>
  );
}
