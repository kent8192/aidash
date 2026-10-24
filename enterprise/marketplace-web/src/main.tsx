import { useEffect, useState, type FormEvent, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import {
  ApiError,
  downloadPath,
  price,
  request,
  write,
  type LibraryEntry,
  type Plugin,
  type PluginVersion,
  type Publisher,
  type Session,
} from "./api";
import "./style.css";

const api = "/api/marketplace";
type Resource<T> = {
  data?: T;
  error?: Error;
  loading: boolean;
  reload: () => void;
};

function useResource<T>(path: string): Resource<T> {
  const [attempt, setAttempt] = useState(0);
  const [state, setState] = useState<{
    path: string;
    attempt: number;
    data?: T;
    error?: Error;
  }>({ path, attempt: -1 });
  useEffect(() => {
    let current = true;
    request<T>(path)
      .then((data) => {
        if (current) setState({ path, attempt, data });
      })
      .catch((error: Error) => {
        if (current) setState({ path, attempt, error });
      });
    return () => {
      current = false;
    };
  }, [path, attempt]);
  const loading = state.path !== path || state.attempt !== attempt;
  return {
    data: state.path === path ? state.data : undefined,
    error: loading ? undefined : state.error,
    loading,
    reload: () => setAttempt((value) => value + 1),
  };
}

function ErrorNotice({ error, retry }: { error: Error; retry?: () => void }) {
  return (
    <div className="notice error" role="alert">
      <div>
        <strong>Unable to complete this request</strong>
        <p>{error.message}</p>
      </div>
      {error instanceof ApiError && error.status === 401 ? (
        <a className="button secondary" href="/auth/login">
          Sign in again
        </a>
      ) : (
        retry && (
          <button className="secondary" onClick={retry}>
            Try again
          </button>
        )
      )}
    </div>
  );
}

function Loading() {
  return (
    <div className="loading" role="status" aria-label="Loading">
      <span />
      <span />
      <span />
      <span className="sr-only">Loading…</span>
    </div>
  );
}

function Empty({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="empty">
      <span className="eyebrow">A place to begin</span>
      <h2>{title}</h2>
      <p>{children}</p>
    </section>
  );
}

function Permissions({ permissions }: { permissions: string[] }) {
  return permissions.length ? (
    <ul className="permissions">
      {permissions.map((permission) => (
        <li key={permission}>
          <code>{permission}</code>
        </li>
      ))}
    </ul>
  ) : (
    <p className="muted">No permissions declared.</p>
  );
}

function PluginList({ plugins }: { plugins: Plugin[] }) {
  return (
    <div className="plugin-list">
      {plugins.map((plugin) => (
        <article className="plugin-row" key={plugin.id}>
          <div className="plugin-glyph" aria-hidden="true">
            {plugin.name.slice(0, 2).toUpperCase()}
          </div>
          <div className="plugin-summary">
            <span className="eyebrow">{plugin.publisher_name}</span>
            <h2>
              <a href={`#/plugins/${encodeURIComponent(plugin.slug)}`}>
                {plugin.name}
              </a>
            </h2>
            <p>{plugin.description}</p>
            <div className="row-meta">
              <span>{plugin.license}</span>
              {plugin.latest_version && <code>v{plugin.latest_version}</code>}
              <span>
                {plugin.declared_permissions.length} declared{" "}
                {plugin.declared_permissions.length === 1
                  ? "permission"
                  : "permissions"}
              </span>
            </div>
          </div>
          <span className="price">{price(plugin)}</span>
        </article>
      ))}
    </div>
  );
}

function Catalog() {
  const catalog = useResource<{ plugins: Plugin[] }>(`${api}/catalog`);
  const [query, setQuery] = useState("");
  const plugins = catalog.data?.plugins.filter((plugin) =>
    `${plugin.name} ${plugin.description} ${plugin.publisher_name}`
      .toLowerCase()
      .includes(query.toLowerCase()),
  );
  return (
    <>
      <div className="page-heading">
        <div>
          <span className="eyebrow accent">Agent Plugins</span>
          <h1>
            Good tools.
            <br />
            More capable agents.
          </h1>
          <p>
            Explore plugins, review what they can access, and bring them into
            your own workspace.
          </p>
        </div>
        <div className="heading-aside">
          <span className="status-dot" />
          Open catalog<span>Public metadata. Explicit permissions.</span>
        </div>
      </div>
      <div className="section-bar">
        <h2>
          Explore the catalog <span>{catalog.data?.plugins.length ?? "—"}</span>
        </h2>
        <label className="search">
          <span className="sr-only">Search plugins</span>
          <input
            type="search"
            placeholder="Search plugins or publishers"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
      </div>
      {catalog.loading ? (
        <Loading />
      ) : catalog.error ? (
        <ErrorNotice error={catalog.error} retry={catalog.reload} />
      ) : !catalog.data?.plugins.length ? (
        <Empty title="The catalog is ready for its first plugin">
          Publish an Agent Plugin to make it discoverable here.{" "}
          <a href="#/publishers">Open Publisher tools</a>.
        </Empty>
      ) : !plugins?.length ? (
        <Empty title="No matching plugins">
          Try another name, publisher, or keyword.
        </Empty>
      ) : (
        <PluginList plugins={plugins} />
      )}
    </>
  );
}

function VersionList({
  versions,
  pluginId,
  downloadable = false,
}: {
  versions: PluginVersion[];
  pluginId: string;
  downloadable?: boolean;
}) {
  return (
    <div className="version-list">
      {versions.map((version) => (
        <article
          className={`version ${version.recalled ? "recalled" : ""}`}
          key={version.version}
        >
          <div className="version-heading">
            <h3>
              <code>{version.version}</code>
            </h3>
            <span className={`badge ${version.recalled ? "danger" : ""}`}>
              {version.recalled ? "Recalled" : "Available"}
            </span>
            <span className="muted">
              {new Intl.NumberFormat("en").format(version.size_bytes)} bytes
            </span>
          </div>
          {version.recalled && (
            <p className="recall-reason">
              {version.recall_reason ||
                "This version must not be used for new Runs."}
            </p>
          )}
          <dl>
            <dt>SHA-256</dt>
            <dd>
              <code className="digest">{version.digest}</code>
            </dd>
            {version.origin && (
              <>
                <dt>Origin</dt>
                <dd className="break">{version.origin}</dd>
              </>
            )}
          </dl>
          {downloadable && !version.recalled && (
            <a
              className="button secondary compact"
              href={downloadPath(pluginId, version.version)}
            >
              Download version
            </a>
          )}
        </article>
      ))}
    </div>
  );
}

function Detail({ slug, session }: { slug: string; session?: Session }) {
  const detail = useResource<{ plugin: Plugin; versions: PluginVersion[] }>(
    `${api}/catalog/${encodeURIComponent(slug)}`,
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<Error>();
  if (detail.loading) return <Loading />;
  if (detail.error)
    return <ErrorNotice error={detail.error} retry={detail.reload} />;
  if (!detail.data) return null;
  const { plugin, versions } = detail.data;
  async function acquire() {
    if (!session) return;
    setError(undefined);
    setBusy(true);
    try {
      await write(
        `${api}/plugins/${encodeURIComponent(plugin.id)}/acquire`,
        {},
        session,
      );
      location.hash = "/library";
    } catch (caught) {
      setError(caught as Error);
    } finally {
      setBusy(false);
    }
  }
  return (
    <>
      <a className="back-link" href="#/">
        ← Explore plugins
      </a>
      <div className="detail-heading">
        <div>
          <span className="eyebrow accent">Agent Plugin</span>
          <h1>{plugin.name}</h1>
          <p>{plugin.description}</p>
        </div>
        <div className="acquire">
          <span className="detail-price">{price(plugin)}</span>
          {!session?.authenticated ? (
            <a className="button" href="/auth/login">
              Sign in to add to Library
            </a>
          ) : plugin.price_cents === 0 ? (
            <button onClick={acquire} disabled={busy}>
              {busy ? "Adding…" : "Add to Library"}
            </button>
          ) : (
            <p className="muted">Paid acquisition is not available yet.</p>
          )}
        </div>
      </div>
      {error && <ErrorNotice error={error} />}
      <div className="detail-grid">
        <section>
          <h2>Versions</h2>
          {versions.length ? (
            <VersionList versions={versions} pluginId={plugin.id} />
          ) : (
            <p className="muted">No published versions.</p>
          )}
        </section>
        <aside className="metadata">
          <h2>Before you add</h2>
          <dl>
            <dt>Publisher</dt>
            <dd>{plugin.publisher_name}</dd>
            <dt>License</dt>
            <dd>{plugin.license}</dd>
            <dt>Origin</dt>
            <dd className="break">{plugin.origin}</dd>
          </dl>
          <h3>Declared Permissions</h3>
          <Permissions permissions={plugin.declared_permissions} />
          <p className="small muted">
            Your Deployment reviews local approval before an Agent version can
            use this plugin.
          </p>
        </aside>
      </div>
    </>
  );
}

function Protected({
  session,
  children,
}: {
  session: Resource<Session>;
  children: (value: Session) => ReactNode;
}) {
  if (session.loading) return <Loading />;
  if (session.error)
    return <ErrorNotice error={session.error} retry={session.reload} />;
  if (!session.data?.authenticated)
    return (
      <Empty title="Your Aidash Account opens the door">
        Sign in to manage your Library and Publishers.{" "}
        <a href="/auth/login">Sign in to Marketplace</a>.
      </Empty>
    );
  return children(session.data);
}

function Library() {
  const library = useResource<{ entries: LibraryEntry[] }>(`${api}/library`);
  return (
    <>
      <div className="page-heading compact-heading">
        <div>
          <span className="eyebrow accent">Your collection</span>
          <h1>Your Library</h1>
          <p>
            Agent Plugins available to your Aidash Account. Your Deployment
            installs and pins its own copy.
          </p>
        </div>
        <a className="button secondary" href="#/">
          Explore plugins
        </a>
      </div>
      {library.loading ? (
        <Loading />
      ) : library.error ? (
        <ErrorNotice error={library.error} retry={library.reload} />
      ) : !library.data?.entries.length ? (
        <Empty title="Make room for your next tool">
          Find a plugin in the public catalog and add it to your Library.{" "}
          <a href="#/">Explore plugins</a>.
        </Empty>
      ) : (
        <div className="library-list">
          {library.data.entries.map((entry) => (
            <section key={entry.plugin_id}>
              <h2>
                <a href={`#/plugins/${encodeURIComponent(entry.slug)}`}>
                  {entry.name}
                </a>
              </h2>
              <VersionList
                versions={entry.versions}
                pluginId={entry.plugin_id}
                downloadable
              />
            </section>
          ))}
        </div>
      )}
    </>
  );
}

function PublisherPlugins({
  publisher,
  session,
}: {
  publisher: Publisher;
  session: Session;
}) {
  const plugins = useResource<{ plugins: Plugin[] }>(
    `${api}/publishers/${encodeURIComponent(publisher.id)}/plugins`,
  );
  const [selected, setSelected] = useState<string>("");
  const [created, setCreated] = useState<Plugin>();
  const [error, setError] = useState<Error>();
  const [busy, setBusy] = useState(false);
  const [showCreate, setShowCreate] = useState(false);
  const available = [
    ...(plugins.data?.plugins ?? []),
    ...(created && !plugins.data?.plugins.some((item) => item.id === created.id)
      ? [created]
      : []),
  ];
  async function create(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true);
    setError(undefined);
    const data = new FormData(event.currentTarget);
    try {
      const result = await write<{ plugin: Plugin }>(
        `${api}/plugins`,
        {
          publisher_id: publisher.id,
          slug: data.get("slug"),
          name: data.get("name"),
          description: data.get("description"),
          license: data.get("license"),
          origin: data.get("origin"),
          price_cents: Number(data.get("price_cents")),
          currency: data.get("currency"),
          declared_permissions: String(data.get("permissions") ?? "")
            .split("\n")
            .map((value) => value.trim())
            .filter(Boolean),
        },
        session,
      );
      setCreated(result.plugin);
      setSelected(result.plugin.id);
      setShowCreate(false);
    } catch (caught) {
      setError(caught as Error);
    } finally {
      setBusy(false);
    }
  }
  const plugin = available.find((item) => item.id === selected);
  return (
    <section className="publisher-workspace">
      <div className="section-bar">
        <h2>{publisher.name}</h2>
        <button
          className="secondary"
          onClick={() => setShowCreate((value) => !value)}
        >
          {showCreate ? "Cancel new listing" : "New plugin listing"}
        </button>
      </div>
      {plugins.loading ? (
        <Loading />
      ) : plugins.error ? (
        <ErrorNotice error={plugins.error} retry={plugins.reload} />
      ) : available.length ? (
        <label className="field">
          Plugin listing
          <select
            value={selected}
            onChange={(event) => setSelected(event.target.value)}
          >
            <option value="">Choose a plugin</option>
            {available.map((item) => (
              <option value={item.id} key={item.id}>
                {item.name}
                {item.latest_version ? ` · ${item.latest_version}` : " · Draft"}
              </option>
            ))}
          </select>
        </label>
      ) : (
        !showCreate && (
          <p className="muted">
            Create your first listing, then publish an immutable version.
          </p>
        )
      )}
      {error && <ErrorNotice error={error} />}
      {showCreate && (
        <form className="form-panel" onSubmit={create}>
          <h3>New plugin listing</h3>
          <div className="form-grid">
            <label className="field">
              Plugin name
              <input name="name" required maxLength={160} />
            </label>
            <label className="field">
              URL slug
              <input
                name="slug"
                required
                pattern="[a-z0-9]+(-[a-z0-9]+)*"
                placeholder="source-reader"
              />
            </label>
            <label className="field wide">
              Description
              <textarea name="description" required rows={3} />
            </label>
            <label className="field">
              License
              <input name="license" required placeholder="Apache-2.0" />
            </label>
            <label className="field">
              Origin URL
              <input
                name="origin"
                type="url"
                required
                pattern="https://.*"
                placeholder="https://example.org/plugin"
              />
            </label>
            <label className="field">
              Price in US cents
              <input
                name="price_cents"
                type="number"
                min="0"
                step="1"
                defaultValue="0"
                required
              />
              <span className="hint">
                0 for free. Paid acquisition is not available yet.
              </span>
            </label>
            <label className="field">
              Currency
              <input name="currency" value="USD" readOnly />
            </label>
            <label className="field wide">
              Declared Permissions
              <textarea
                name="permissions"
                rows={3}
                placeholder="One permission per line"
              />
            </label>
          </div>
          <button disabled={busy}>
            {busy ? "Creating…" : "Create listing"}
          </button>
        </form>
      )}
      {plugin && (
        <PublishVersion
          key={plugin.id}
          plugin={plugin}
          session={session}
          onPublished={plugins.reload}
        />
      )}
    </section>
  );
}

function PublishVersion({
  plugin,
  session,
  onPublished,
}: {
  plugin: Plugin;
  session: Session;
  onPublished: () => void;
}) {
  const [error, setError] = useState<Error>();
  const [result, setResult] = useState<PluginVersion>();
  const [busy, setBusy] = useState(false);
  async function publish(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(undefined);
    setResult(undefined);
    setBusy(true);
    const data = new FormData(event.currentTarget);
    try {
      const file = data.get("bundle");
      if (!(file instanceof File) || !file.size)
        throw new Error("Choose an Agent Plugin bundle.");
      if (file.size > 12 * 1024 * 1024)
        throw new Error("The bundle must be 12 MiB or smaller.");
      const parsed: unknown = JSON.parse(await file.text());
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed))
        throw new Error("The bundle must map file paths to base64 contents.");
      const entries = Object.entries(parsed);
      if (
        !entries.length ||
        entries.some(
          ([path, value]) =>
            !/^[\x20-\x7e]+$/.test(path) || typeof value !== "string",
        )
      )
        throw new Error(
          "Bundle paths must be ASCII and contents must be base64 strings.",
        );
      const files = Object.fromEntries(
        entries.sort(([left], [right]) =>
          left < right ? -1 : left > right ? 1 : 0,
        ),
      );
      const payload = { version: data.get("version"), files };
      if (
        new TextEncoder().encode(JSON.stringify(payload)).byteLength >
        12 * 1024 * 1024
      )
        throw new Error(
          "The complete publication request must be 12 MiB or smaller.",
        );
      const response = await write<{ version: PluginVersion }>(
        `${api}/plugins/${encodeURIComponent(plugin.id)}/versions`,
        payload,
        session,
      );
      setResult(response.version);
      onPublished();
    } catch (caught) {
      setError(caught as Error);
    } finally {
      setBusy(false);
    }
  }
  return (
    <form className="form-panel" onSubmit={publish}>
      <span className="eyebrow">Publish an immutable version</span>
      <h3>{plugin.name}</h3>
      <p className="muted">
        Published versions cannot be overwritten. Review the bundle and its
        declared permissions before publishing.
      </p>
      <div className="form-grid">
        <label className="field">
          Version
          <input name="version" placeholder="1.0.0" required />
        </label>
        <label className="field">
          Agent Plugin bundle
          <input
            name="bundle"
            type="file"
            accept=".json,application/json"
            required
          />
          <span className="hint">
            JSON file map including root plugin.json; ASCII paths and base64
            contents.
          </span>
        </label>
      </div>
      {error && <ErrorNotice error={error} />}
      {result && (
        <div className="notice success" role="status">
          Version {result.version} published.{" "}
          <code className="digest">{result.digest}</code>
        </div>
      )}
      <button disabled={busy}>
        {busy ? "Validating and publishing…" : "Publish version"}
      </button>
    </form>
  );
}

function Publishers({ session }: { session: Session }) {
  const publishers = useResource<{ publishers: Publisher[] }>(
    `${api}/publishers`,
  );
  const [created, setCreated] = useState<Publisher[]>([]);
  const [selected, setSelected] = useState("");
  const [error, setError] = useState<Error>();
  const [busy, setBusy] = useState(false);
  const all = [
    ...(publishers.data?.publishers ?? []),
    ...created.filter(
      (item) =>
        !publishers.data?.publishers.some(
          (existing) => existing.id === item.id,
        ),
    ),
  ];
  const current = all.find((item) => item.id === selected) ?? all[0];
  async function create(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(undefined);
    setBusy(true);
    const form = event.currentTarget;
    try {
      const response = await write<{ publisher: Publisher }>(
        `${api}/publishers`,
        { name: new FormData(form).get("name") },
        session,
      );
      setCreated((items) => [...items, response.publisher]);
      setSelected(response.publisher.id);
      form.reset();
    } catch (caught) {
      setError(caught as Error);
    } finally {
      setBusy(false);
    }
  }
  return (
    <>
      <div className="page-heading compact-heading">
        <div>
          <span className="eyebrow accent">For builders</span>
          <h1>Publisher tools</h1>
          <p>
            Publish plugins under your own Publisher. Your Aidash Account owns
            these Publisher profiles.
          </p>
        </div>
      </div>
      {publishers.loading ? (
        <Loading />
      ) : publishers.error ? (
        <ErrorNotice error={publishers.error} retry={publishers.reload} />
      ) : (
        <>
          {all.length > 1 && (
            <label className="field">
              Active Publisher
              <select
                value={current?.id ?? ""}
                onChange={(event) => setSelected(event.target.value)}
              >
                {all.map((item) => (
                  <option value={item.id} key={item.id}>
                    {item.name}
                  </option>
                ))}
              </select>
            </label>
          )}
          {current && (
            <PublisherPlugins
              key={current.id}
              publisher={current}
              session={session}
            />
          )}
          <form className="form-panel publisher-create" onSubmit={create}>
            <h2>Create a Publisher</h2>
            <p className="muted">
              Give your plugins a recognizable publishing identity.
            </p>
            <label className="field">
              Publisher name
              <input name="name" required maxLength={160} />
            </label>
            {error && <ErrorNotice error={error} />}
            <button disabled={busy}>
              {busy ? "Creating…" : "Create Publisher"}
            </button>
          </form>
        </>
      )}
    </>
  );
}

function App() {
  const [route, setRoute] = useState(location.hash.slice(1) || "/");
  const session = useResource<Session>(`${api}/session`);
  const [logoutError, setLogoutError] = useState<Error>();
  const [loggingOut, setLoggingOut] = useState(false);
  useEffect(() => {
    const update = () => {
      setRoute(location.hash.slice(1) || "/");
      window.scrollTo(0, 0);
    };
    window.addEventListener("hashchange", update);
    return () => window.removeEventListener("hashchange", update);
  }, []);
  async function logout() {
    if (!session.data) return;
    setLoggingOut(true);
    setLogoutError(undefined);
    try {
      await write("/auth/logout", {}, session.data);
      session.reload();
      location.hash = "/";
    } catch (caught) {
      setLogoutError(caught as Error);
    } finally {
      setLoggingOut(false);
    }
  }
  let content: ReactNode;
  if (route === "/" || route === "") content = <Catalog />;
  else if (route.startsWith("/plugins/")) {
    let slug: string;
    try {
      slug = decodeURIComponent(route.slice("/plugins/".length));
    } catch {
      slug = "";
    }
    content = <Detail key={slug} slug={slug} session={session.data} />;
  } else if (route === "/library")
    content = <Protected session={session}>{() => <Library />}</Protected>;
  else if (route === "/publishers")
    content = (
      <Protected session={session}>
        {(value) => <Publishers session={value} />}
      </Protected>
    );
  else
    content = (
      <Empty title="This page could not be found">
        <a href="#/">Return to the catalog</a>.
      </Empty>
    );
  return (
    <>
      <a
        href="#main"
        className="skip-link"
        onClick={(event) => {
          event.preventDefault();
          const main = document.getElementById("main");
          main?.focus();
          main?.scrollIntoView();
        }}
      >
        Skip to content
      </a>
      <header className="site-header">
        <a className="brand" href="#/" aria-label="Aidash Marketplace home">
          <span className="brand-mark" aria-hidden="true">
            a
          </span>
          <strong>aidash</strong>
          <span className="brand-divider" />
          <span>marketplace</span>
        </a>
        <nav aria-label="Main navigation">
          <a href="#/" aria-current={route === "/" ? "page" : undefined}>
            Explore
          </a>
          <a
            href="#/library"
            aria-current={route === "/library" ? "page" : undefined}
          >
            Library
          </a>
          <a
            href="#/publishers"
            aria-current={route === "/publishers" ? "page" : undefined}
          >
            Publishers
          </a>
        </nav>
        <div className="account-actions">
          {session.data?.authenticated ? (
            <>
              <a className="button secondary compact" href="/auth/account">
                Account settings
              </a>
              <button
                className="secondary compact"
                onClick={logout}
                disabled={loggingOut}
              >
                {loggingOut ? "Signing out…" : "Sign out"}
              </button>
            </>
          ) : (
            <a className="button secondary compact" href="/auth/login">
              Sign in
            </a>
          )}
        </div>
      </header>
      <main id="main" className="main" tabIndex={-1}>
        {logoutError && <ErrorNotice error={logoutError} />}
        {session.error && route !== "/library" && route !== "/publishers" && (
          <div className="notice" role="status">
            Sign-in status is unavailable. The public catalog is still
            accessible.{" "}
            <button className="text-button" onClick={session.reload}>
              Retry sign-in status
            </button>
          </div>
        )}
        {content}
      </main>
      <footer className="site-footer">
        <span>Aidash Marketplace</span>
        <span>Discover · Review · Make it yours</span>
      </footer>
    </>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
