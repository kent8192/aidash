import { Notice } from "./components/patterns";
import { useNodes } from "./nodes";
import { useI18n } from "./ui";
import {
  unconfirmedNodeName,
  unconfirmedText,
  type UnconfirmedLine,
} from "./unconfirmed";

/**
 * Right above a shortened list: one warning line for each Node that did not
 * answer, with how many items of this list were left out. A caller computes
 * `lines` with `unconfirmedFor`, shows its empty text only when there are none
 * (that text would be false), and renders nothing here otherwise.
 */
export function UnconfirmedNotice({
  lines,
  className,
}: {
  lines: readonly UnconfirmedLine[];
  className?: string;
}) {
  const { t } = useI18n();
  const { nodes } = useNodes();
  if (lines.length === 0) return null;
  return (
    <Notice
      tone="warning"
      role="status"
      data-testid="unconfirmed"
      className={className}
    >
      {lines.map(({ node, count }) => (
        <p key={node}>
          {unconfirmedText(
            t("unconfirmedItems"),
            count,
            unconfirmedNodeName(nodes, node),
          )}
        </p>
      ))}
    </Notice>
  );
}
