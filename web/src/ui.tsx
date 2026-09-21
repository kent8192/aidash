import {
  createContext,
  cloneElement,
  useId,
  type ReactElement,
  useContext,
  useEffect,
  useRef,
  type ReactNode,
} from "react";
import en from "./locales/en-US.json";
import ja from "./locales/ja-JP.json";
export type Locale = "en-US" | "ja-JP";
export const LocaleContext = createContext<Locale>("ja-JP");
export function useI18n() {
  const locale = useContext(LocaleContext);
  const dictionary: Record<string, string> = locale === "ja-JP" ? ja : en;
  return {
    locale,
    t: (key: string) => dictionary[key] ?? key,
    local: (value: Record<string, string>) =>
      value[locale] ??
      value[locale.slice(0, 2)] ??
      value.en ??
      Object.values(value)[0] ??
      "",
  };
}
export function Badge({ value }: { value: string }) {
  const { t } = useI18n();
  return <span className={`badge ${value.toLowerCase()}`}>{t(value)}</span>;
}
export function JsonView({ value }: { value: unknown }) {
  return (
    <pre className="json">
      {typeof value === "string" ? value : JSON.stringify(value, null, 2)}
    </pre>
  );
}
export function Empty() {
  const { t } = useI18n();
  return (
    <div className="empty">
      <span className="empty-mark">◇</span>
      <h3>{t("empty")}</h3>
      <p>{t("emptyHelp")}</p>
    </div>
  );
}
export function Field({
  label,
  children,
}: {
  label: string;
  children: ReactElement<{ id?: string }>;
}) {
  const generatedId = useId();
  const id = children.props.id ?? generatedId;
  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      {cloneElement(children, { id })}
    </div>
  );
}
export function Modal({
  title,
  children,
  close,
}: {
  title: string;
  children: ReactNode;
  close: () => void;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const { t } = useI18n();
  useEffect(() => {
    ref.current?.showModal();
  }, []);
  return (
    <dialog ref={ref} onCancel={close}>
      <div className="modal-heading">
        <h2>{title}</h2>
        <button className="icon-button" onClick={close} aria-label={t("close")}>
          ×
        </button>
      </div>
      {children}
    </dialog>
  );
}
export function Panel({
  title,
  action,
  children,
}: {
  title: string;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="panel">
      <div className="panel-heading">
        <h2>{title}</h2>
        {action}
      </div>
      {children}
    </section>
  );
}
