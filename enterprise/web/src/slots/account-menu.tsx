import { ShieldCheck } from "lucide-react";
import { desktopAvailable } from "@/desktop";
import { useI18n } from "@/ui";
import { markEnrollIntent } from "../gcip-intent";
import type { AccountEntriesProps } from "@/slots/types";

const setUp = {
  "ja-JP": "2 段階認証を設定",
  "en-US": "Set up two-step verification",
};

export function AccountEntries({ provider, mappings }: AccountEntriesProps) {
  const { locale } = useI18n();
  // Enrollment signs in again through the GCIP Tenant of this browser
  // session's External Identity, which the server reads from the session. The
  // desktop app signs in through the system browser, so it enrolls there too;
  // only the web page can start it from here.
  if (provider !== "gcip" || desktopAvailable() || mappings.length === 0)
    return null;
  return (
    <div className="grid gap-px border-t border-border pt-1.5">
      <button
        type="button"
        onClick={() => {
          markEnrollIntent();
          const query = new URLSearchParams({
            return_to: window.location.pathname + window.location.search,
          });
          window.location.assign(`/auth/login?${query}`);
        }}
        className="flex h-8 items-center gap-2.5 rounded-md px-2 text-left text-muted-foreground transition-colors duration-150 hover:bg-accent hover:text-foreground"
      >
        <ShieldCheck aria-hidden className="size-3.5" />
        {setUp[locale]}
      </button>
    </div>
  );
}
