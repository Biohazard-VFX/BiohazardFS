import React, { useCallback, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import './globals.css';

type DaemonStatusResult = Awaited<ReturnType<typeof window.biohazardfs.daemonStatus>>;
type VersionInfo = Awaited<ReturnType<typeof window.biohazardfs.versions>>;

function App() {
  const [daemon, setDaemon] = useState<DaemonStatusResult | null>(null);
  const [versions, setVersions] = useState<VersionInfo | null>(null);

  const refresh = useCallback(async () => {
    setDaemon(await window.biohazardfs.daemonStatus());
  }, []);

  useEffect(() => {
    void window.biohazardfs.daemonStatus().then((status) => {
      setDaemon(status);
    });
    void window.biohazardfs.versions().then((versionInfo) => {
      setVersions(versionInfo);
    });
  }, []);

  const daemonState = daemon?.ok ? 'Connected' : 'Waiting for daemon';

  return (
    <main className="app-shell">
      <section className="hero-panel">
        <p className="eyebrow">BiohazardFS client scaffold</p>
        <h1>Biohazard Workspace</h1>
        <p className="lede">
          Desktop shell, CLI, and daemon foundations are wired together. This is not a production
          sync client yet.
        </p>
      </section>

      <section className="grid">
        <article className="card">
          <div className="card-header">
            <h2>Daemon</h2>
            <span className={daemon?.ok ? 'pill good' : 'pill warn'}>{daemonState}</span>
          </div>
          <dl>
            <dt>Endpoint</dt>
            <dd>{daemon?.endpoint ?? '127.0.0.1:47666'}</dd>
            <dt>Status</dt>
            <dd>{daemon?.ok ? 'ready' : (daemon?.error ?? 'not checked yet')}</dd>
          </dl>
          <button
            type="button"
            onClick={() => {
              void refresh();
            }}
          >
            Refresh daemon status
          </button>
        </article>

        <article className="card">
          <div className="card-header">
            <h2>Workspace</h2>
            <span className="pill muted">Stub</span>
          </div>
          <ul className="checklist">
            <li>Mount status panel placeholder</li>
            <li>Cache status panel placeholder</li>
            <li>Transfer queue placeholder</li>
            <li>Conflict/problem panel placeholder</li>
          </ul>
        </article>

        <article className="card wide">
          <div className="card-header">
            <h2>Runtime</h2>
            <span className="pill muted">Local</span>
          </div>
          <pre>{JSON.stringify({ versions, daemon }, null, 2)}</pre>
        </article>
      </section>
    </main>
  );
}

const root = document.getElementById('root');
if (!root) {
  throw new Error('missing root element');
}

createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
