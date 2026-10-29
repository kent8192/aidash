import { UserRoundCog } from "lucide-react";
import { Button } from "./components/ui/button";
import { OPEN_CONTEXT_SWITCHER } from "./collaboration/model";
import { Notice } from "./components/patterns";
import { useI18n } from "./ui";

/**
 * Shown in place of Tenant content while the person acts as a Deployment
 * Administrator. The role grants no Tenant content authority (ADR 0102), so
 * the server refuses these routes; the way forward is a Tenant Mapping.
 */
export function AdministratorTenantNotice({
  className = "p-4 md:p-6",
}: {
  className?: string;
}) {
  const { t } = useI18n();
  return (
    <div className={className}>
      <Notice role="status">
        <div className="grid max-w-xl justify-items-start gap-3">
          <p className="font-medium">{t("administratorNoTenantContent")}</p>
          <p>{t("administratorSwitchToMapping")}</p>
          <Button
            variant="outline"
            size="sm"
            type="button"
            onClick={() =>
              window.dispatchEvent(new Event(OPEN_CONTEXT_SWITCHER))
            }
          >
            <UserRoundCog aria-hidden />
            {t("administratorOpenSwitcher")}
          </Button>
        </div>
      </Notice>
    </div>
  );
}
