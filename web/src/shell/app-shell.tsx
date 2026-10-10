import {
  forwardRef,
  useEffect,
  useRef,
  useState,
  type ComponentPropsWithoutRef,
  type ReactNode,
} from "react";
import { Link } from "@tanstack/react-router";
import {
  Bot,
  ChevronDown,
  ChevronRight,
  FolderPlus,
  KeyRound,
  Menu,
  MessagesSquare,
  Moon,
  Network,
  Package,
  PencilRuler,
  Plus,
  Rocket,
  Server,
  Settings2,
  ShieldHalf,
  Store,
  Sun,
  Waypoints,
} from "lucide-react";
import aidashIcon from "../assets/brand/aidash-app-icon.svg?no-inline";
import { Button } from "../components/ui/button";
import { Kbd } from "../components/ui/kbd";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "../components/ui/popover";
import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
} from "../components/ui/sheet";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "../components/ui/tooltip";
import { collaborationCopy } from "../collaboration/copy";
import {
  visibleSettingsSections,
  type Destination,
  type SettingsSection,
} from "../collaboration/model";
import { cn } from "../lib/utils";
import { NotificationBell } from "../notifications";
import type { Theme } from "../theme";
import type { State } from "../types";
import { useI18n } from "../ui";
import { AccountAvatar, AccountPanel, type Account } from "./account-menu";
import { shellCopy } from "./copy";
import { DecisionQueue, type PendingDecision } from "./decision-queue";
import { WorkspaceList } from "./workspace-list";

export type Go = (
  section: Destination,
  context?: { channel?: string; focus?: string; settings?: SettingsSection },
) => void;

const destinations = [
  { section: "collaboration", icon: MessagesSquare, key: "1" },
  { section: "graph", icon: Waypoints, key: "2" },
  { section: "creator", icon: PencilRuler, key: "3" },
  { section: "trust", icon: ShieldHalf, key: "4" },
] as const;
/** Settings views limited to operators; the server still enforces access. */
const operatorSettings: Partial<Record<SettingsSection, true>> = {
  deployment: true,
};
const settingsIcons: Record<
  (typeof visibleSettingsSections)[number],
  typeof Server
> = {
  node: Server,
  agents: Bot,
  registry: Package,
  clusters: Network,
  deployment: Rocket,
  providerCredentials: KeyRound,
  marketplace: Store,
};
const railButton =
  "relative grid size-10 shrink-0 place-items-center rounded-md text-muted-foreground transition-colors duration-150 hover:bg-surface hover:text-foreground aria-expanded:bg-surface aria-expanded:text-foreground [&_svg]:size-[18px]";
const menuItem =
  "flex h-8 w-full items-center gap-2.5 rounded-md px-2 text-left text-[13px] text-foreground transition-colors duration-150 hover:bg-accent aria-[current=page]:bg-brand-soft [&_svg]:size-[15px] [&_svg]:shrink-0 [&_svg]:text-muted-foreground";

function useSectionLabel() {
  const { locale } = useI18n();
  const copy = shellCopy[locale];
  const labels: Record<Destination, string> = {
    collaboration: copy.request,
    graph: collaborationCopy[locale].graph,
    creator: copy.creator,
    trust: copy.trust,
    settings: collaborationCopy[locale].settings,
  };
  return labels;
}

const RailButton = forwardRef<
  HTMLButtonElement,
  ComponentPropsWithoutRef<"button"> & { label: string; hint?: string }
>(({ label, hint, className, children, ...props }, ref) => (
  <Tooltip>
    <TooltipTrigger asChild>
      <button
        ref={ref}
        type="button"
        aria-label={label}
        className={cn(railButton, className)}
        {...props}
      >
        {children}
      </button>
    </TooltipTrigger>
    <TooltipContent side="right">
      {label}
      {hint && <Kbd>{hint}</Kbd>}
    </TooltipContent>
  </Tooltip>
));
RailButton.displayName = "RailButton";

function SettingsLinks({
  operator,
  current,
  select,
  prepare,
}: {
  operator: boolean;
  current?: SettingsSection;
  select: (settings: SettingsSection) => void;
  prepare: () => void;
}) {
  const { locale, t } = useI18n();
  const copy = shellCopy[locale];
  return (
    <div className="grid gap-px">
      {visibleSettingsSections
        .filter((value) => operator || !operatorSettings[value])
        .map((value) => {
          const Icon = settingsIcons[value];
          return (
            <button
              key={value}
              type="button"
              className={menuItem}
              aria-current={current === value ? "page" : undefined}
              onClick={() => select(value)}
            >
              <Icon aria-hidden />
              {value === "node" ? copy.general : t(value)}
            </button>
          );
        })}
      <div className="my-1 border-t border-border" />
      <button type="button" className={menuItem} onClick={prepare}>
        <FolderPlus aria-hidden />
        {collaborationCopy[locale].prepare}
      </button>
    </div>
  );
}

/** Application frame: icon rail, context bar, mobile menu and the main landmark. */
export function AppShell({
  section,
  settings,
  data,
  channel,
  operator,
  streamStatus,
  decisions,
  selectDecision,
  go,
  newRequest,
  prepareChannel,
  account,
  theme,
  children,
}: {
  section: Destination;
  settings: SettingsSection;
  data?: State;
  channel: string;
  operator: boolean;
  streamStatus: "live" | "reconnecting";
  decisions: PendingDecision[];
  selectDecision: (decision: PendingDecision) => void;
  go: Go;
  newRequest: () => void;
  prepareChannel: () => void;
  account: Account;
  theme: Theme;
  children: ReactNode;
}) {
  const { locale } = useI18n();
  const copy = shellCopy[locale];
  const collab = collaborationCopy[locale];
  const sectionLabel = useSectionLabel();
  const [filter, setFilter] = useState("");
  const [switcherOpen, setSwitcherOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [accountOpen, setAccountOpen] = useState(false);
  const [mobileOpen, setMobileOpen] = useState(false);
  const searchInput = useRef<HTMLInputElement>(null);
  const focusSearch = useRef(false);
  const closeMenus = () => {
    setSwitcherOpen(false);
    setSettingsOpen(false);
    setAccountOpen(false);
    setMobileOpen(false);
  };
  const navigate: Go = (target, context) => {
    closeMenus();
    go(target, context);
  };
  const latest = useRef({ navigate, section });
  useEffect(() => {
    latest.current = { navigate, section };
  });
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        focusSearch.current = true;
        if (window.matchMedia("(min-width: 768px)").matches)
          setSwitcherOpen(true);
        else setMobileOpen(true);
        requestAnimationFrame(() => searchInput.current?.focus());
        return;
      }
      if (event.metaKey || event.ctrlKey || event.altKey || event.repeat)
        return;
      const target = event.target as HTMLElement | null;
      if (
        target?.closest(
          "input, textarea, select, [contenteditable], [role=dialog], [role=menu], [role=listbox]",
        ) ||
        document.querySelector("[role=dialog]")
      )
        return;
      const destination = destinations.find((item) => item.key === event.key);
      if (destination && destination.section !== latest.current.section)
        latest.current.navigate(destination.section, { focus: "" });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  const workspace = data?.workspaces.find((value) => value.id === channel);
  const nodeName = data?.node.id.replace(/^aidash:\/\//, "");
  const create = () => {
    closeMenus();
    newRequest();
  };
  const prepare = () => {
    closeMenus();
    prepareChannel();
  };
  const focusPanel = (event: Event) => {
    // Keep focus inside the flyout without painting a focus ring on its first control.
    event.preventDefault();
    (event.currentTarget as HTMLElement).focus();
  };
  const menuAccount: Account = {
    ...account,
    chooseContext: (value) => {
      closeMenus();
      account.chooseContext(value);
    },
  };
  const listProps = {
    data,
    current: section === "collaboration" ? channel : "",
    filter,
    setFilter,
    searchInput,
    create,
    navigate: closeMenus,
  };
  const destinationButton = (
    item: (typeof destinations)[number],
    inSheet: boolean,
  ) => {
    const Icon = item.icon;
    const current = section === item.section;
    const props = {
      "aria-current": current ? ("page" as const) : undefined,
      onClick: () => navigate(item.section, { focus: "" }),
    };
    return inSheet ? (
      <button key={item.section} type="button" className={menuItem} {...props}>
        <Icon aria-hidden />
        {sectionLabel[item.section]}
      </button>
    ) : (
      <RailButton
        key={item.section}
        label={sectionLabel[item.section]}
        hint={item.key}
        className={cn(
          current &&
            "bg-raised text-brand before:absolute before:inset-y-[11px] before:-left-2 before:w-0.5 before:rounded-r-sm before:bg-brand-mark hover:bg-raised hover:text-brand",
        )}
        {...props}
      >
        <Icon aria-hidden strokeWidth={1.75} />
      </RailButton>
    );
  };
  return (
    <div className="flex min-h-0 w-full flex-1 overflow-hidden bg-background text-foreground">
      <a
        href="#main"
        className="sr-only z-[60] rounded-md bg-primary px-3 py-2 text-primary-foreground focus:not-sr-only focus:fixed focus:left-3 focus:top-3"
      >
        {copy.skip}
      </a>
      <div className="relative z-40 hidden w-14 shrink-0 flex-col items-center gap-1 border-r border-border bg-rail pb-3.5 pt-2.5 md:flex">
        <Link
          to="/$section"
          params={{ section: "collaboration" }}
          search={{ channel: channel || undefined }}
          onClick={closeMenus}
          className="mb-2 grid size-8 place-items-center rounded-md"
        >
          <img
            src={aidashIcon}
            alt="Aidash"
            width={32}
            height={32}
            className="size-8 rounded-md"
          />
        </Link>
        <RailButton
          label={copy.newRequest}
          onClick={create}
          className="border border-border-strong text-foreground"
        >
          <Plus aria-hidden strokeWidth={1.75} />
        </RailButton>
        <span aria-hidden className="my-1.5 h-px w-5 bg-border" />
        <nav aria-label={copy.primaryNavigation} className="contents">
          {destinations.map((item) => destinationButton(item, false))}
        </nav>
        <div className="mt-auto flex flex-col items-center gap-1">
          <NotificationBell className={railButton} />
          <Popover open={settingsOpen} onOpenChange={setSettingsOpen}>
            <PopoverTrigger asChild>
              <RailButton
                label={collab.settings}
                className={cn(
                  section === "settings" && "bg-raised text-brand",
                )}
              >
                <Settings2 aria-hidden strokeWidth={1.75} />
              </RailButton>
            </PopoverTrigger>
            <PopoverContent
              side="right"
              align="end"
              sideOffset={14}
              onOpenAutoFocus={focusPanel}
              className="w-64 p-1.5"
            >
              <div className="flex items-center justify-between gap-2 px-2 pb-2 pt-1.5">
                <span className="text-[13px] font-semibold">
                  {collab.settings}
                </span>
                <span className="truncate font-mono text-[11px] text-faint">
                  {nodeName}
                </span>
              </div>
              <SettingsLinks
                operator={operator}
                current={section === "settings" ? settings : undefined}
                select={(value) => navigate("settings", { settings: value })}
                prepare={prepare}
              />
            </PopoverContent>
          </Popover>
          <Popover open={accountOpen} onOpenChange={setAccountOpen}>
            <PopoverTrigger
              aria-label={copy.account}
              className="mt-2 rounded-md transition-opacity duration-150 hover:opacity-85"
            >
              <AccountAvatar name={account.name} />
            </PopoverTrigger>
            <PopoverContent
              side="right"
              align="end"
              sideOffset={14}
              onOpenAutoFocus={focusPanel}
              className="w-[300px] p-1.5"
            >
              <AccountPanel account={menuAccount} />
            </PopoverContent>
          </Popover>
        </div>
      </div>
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        <header className="relative z-30 flex h-11 shrink-0 items-center gap-2 border-b border-border bg-background pl-1.5 pr-2 md:gap-4 md:pl-3.5 md:pr-3">
          <Button
            variant="ghost"
            size="icon"
            className="md:hidden"
            aria-label={collab.channels}
            aria-expanded={mobileOpen}
            onClick={() => setMobileOpen(true)}
          >
            <Menu aria-hidden className="size-4" />
          </Button>
          <nav
            aria-label={copy.location}
            className="flex min-w-0 items-center gap-1 text-xs"
          >
            {nodeName && (
              <>
                <span
                  className="hidden h-7 items-center gap-1.5 px-2 text-muted-foreground md:inline-flex"
                  title={data?.node.id}
                >
                  <Server aria-hidden className="size-3.5" />
                  <span className="font-mono">{nodeName}</span>
                </span>
                <ChevronRight
                  aria-hidden
                  className="hidden size-3.5 shrink-0 text-faint md:block"
                />
              </>
            )}
            <Popover
              open={switcherOpen}
              onOpenChange={(open) => {
                focusSearch.current = open;
                setSwitcherOpen(open);
              }}
            >
              <PopoverTrigger
                aria-label={`${copy.switchRequest}: ${workspace?.title ?? copy.chooseRequest}`}
                aria-keyshortcuts="Meta+K Control+K"
                className="inline-flex h-7 min-w-0 items-center gap-1.5 rounded-md border border-border px-2 text-foreground transition-colors duration-150 hover:bg-surface aria-expanded:bg-surface"
              >
                <span
                  aria-hidden
                  className={cn(
                    "size-1.5 shrink-0 rounded-full",
                    workspace ? "bg-brand-mark" : "bg-faint",
                  )}
                />
                <span
                  className={cn(
                    "truncate",
                    workspace ? "font-mono" : "text-muted-foreground",
                  )}
                >
                  {workspace?.title ?? copy.chooseRequest}
                </span>
                <ChevronDown
                  aria-hidden
                  className="size-3.5 shrink-0 text-faint"
                />
              </PopoverTrigger>
              <PopoverContent
                align="start"
                className="w-[min(400px,calc(100vw-1.5rem))] p-1.5"
                onOpenAutoFocus={(event) => {
                  event.preventDefault();
                  searchInput.current?.focus();
                }}
              >
                <WorkspaceList
                  {...listProps}
                  showCreate
                  className="max-h-[min(70vh,520px)]"
                />
              </PopoverContent>
            </Popover>
            <ChevronRight
              aria-hidden
              className="hidden size-3.5 shrink-0 text-faint md:block"
            />
            <span className="hidden whitespace-nowrap px-1.5 font-medium md:inline">
              {sectionLabel[section]}
            </span>
          </nav>
          <div className="ml-auto flex shrink-0 items-center gap-1.5 md:gap-3">
            <span
              className={cn(
                "stream-status inline-flex items-center gap-1.5 whitespace-nowrap px-1 text-xs",
                streamStatus,
                streamStatus === "live" ? "text-brand" : "text-warning",
              )}
            >
              <span
                aria-hidden
                className={cn(
                  "size-1.5 rounded-full",
                  streamStatus === "live"
                    ? "animate-pulse bg-brand-mark motion-reduce:animate-none"
                    : "bg-warning",
                )}
              />
              <span className="max-md:sr-only">{collab[streamStatus]}</span>
            </span>
            <span aria-hidden className="hidden h-[18px] w-px bg-border md:block" />
            <Button
              variant="ghost"
              size="icon"
              className="size-7"
              aria-label={theme === "dark" ? copy.toLight : copy.toDark}
              onClick={() =>
                account.setPreference(theme === "dark" ? "light" : "dark")
              }
            >
              {theme === "dark" ? (
                <Sun aria-hidden className="size-[15px]" />
              ) : (
                <Moon aria-hidden className="size-[15px]" />
              )}
            </Button>
            <DecisionQueue
              data={data}
              decisions={decisions}
              select={(decision) => {
                closeMenus();
                selectDecision(decision);
              }}
            />
            <NotificationBell className="md:hidden" />
          </div>
        </header>
        <main
          id="main"
          tabIndex={-1}
          className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden outline-none"
        >
          {children}
        </main>
      </div>
      <Sheet
        open={mobileOpen}
        onOpenChange={(open) => {
          if (!open) focusSearch.current = false;
          setMobileOpen(open);
        }}
      >
        <SheetContent
          side="left"
          closeLabel={copy.closeHistory}
          aria-describedby={undefined}
          className="w-[min(340px,88vw)] gap-3 overflow-y-auto bg-rail p-3 [&>*]:shrink-0"
          onOpenAutoFocus={(event) => {
            if (!focusSearch.current) return;
            event.preventDefault();
            searchInput.current?.focus();
          }}
        >
          <SheetHeader className="flex-row items-center gap-2.5">
            <img
              src={aidashIcon}
              alt=""
              width={28}
              height={28}
              className="size-7 rounded-md"
            />
            <SheetTitle>{copy.menu}</SheetTitle>
          </SheetHeader>
          <Button className="justify-start" onClick={create}>
            <Plus />
            {copy.newRequest}
          </Button>
          <nav aria-label={copy.primaryNavigation} className="grid gap-px">
            {destinations.map((item) => destinationButton(item, true))}
          </nav>
          <div className="border-t border-border pt-2">
            <WorkspaceList {...listProps} showCreate={false} />
          </div>
          <div className="grid gap-1 border-t border-border pt-2">
            <p className="px-2 text-[11px] text-faint">{collab.settings}</p>
            <SettingsLinks
              operator={operator}
              current={section === "settings" ? settings : undefined}
              select={(value) => navigate("settings", { settings: value })}
              prepare={prepare}
            />
          </div>
          <div className="border-t border-border pt-1">
            <AccountPanel account={menuAccount} />
          </div>
        </SheetContent>
      </Sheet>
    </div>
  );
}
