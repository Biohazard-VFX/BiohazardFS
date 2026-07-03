import React, { useCallback, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import './globals.css';

type DaemonStatusResult = Awaited<ReturnType<typeof window.biohazardfs.daemonStatus>>;
type VersionInfo = Awaited<ReturnType<typeof window.biohazardfs.versions>>;
type WorkspaceEntry = { name: string; kind: string; size_bytes?: number | null };

function daemonData(result: DaemonStatusResult | null): Record<string, unknown> | null {
  const body = result?.body as { data?: Record<string, unknown> } | undefined;
  return body?.data ?? null;
}

function displayValue(value: unknown, fallback: string): string {
  if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') {
    return String(value);
  }
  return fallback;
}

function App() {
  const [daemon, setDaemon] = useState<DaemonStatusResult | null>(null);
  const [versions, setVersions] = useState<VersionInfo | null>(null);
  const [workspace, setWorkspace] = useState<DaemonStatusResult | null>(null);
  const [workspaceList, setWorkspaceList] = useState<DaemonStatusResult | null>(null);

  const refresh = useCallback(async () => {
    const [daemonStatus, workspaceStatus, list] = await Promise.all([
      window.biohazardfs.daemonStatus(),
      window.biohazardfs.workspaceStatus(),
      window.biohazardfs.workspaceList(''),
    ]);
    setDaemon(daemonStatus);
    setWorkspace(workspaceStatus);
    setWorkspaceList(list);
  }, []);

  useEffect(() => {
    let cancelled = false;
    void Promise.all([
      window.biohazardfs.daemonStatus(),
      window.biohazardfs.workspaceStatus(),
      window.biohazardfs.workspaceList(''),
      window.biohazardfs.versions(),
    ]).then(([daemonStatus, workspaceStatus, list, versionInfo]) => {
      if (cancelled) {
        return;
      }
      setDaemon(daemonStatus);
      setWorkspace(workspaceStatus);
      setWorkspaceList(list);
      setVersions(versionInfo);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const daemonState = daemon?.ok ? 'Connected' : 'Waiting for daemon';
  const workspaceData = daemonData(workspace);
  const listData = daemonData(workspaceList);
  const entries = (Array.isArray(listData?.entries) ? listData.entries : []) as WorkspaceEntry[];
  const workspaceReady = workspaceData?.state === 'ready';

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
            <span className={workspaceReady ? 'pill good' : 'pill warn'}>
              {workspaceReady ? 'Visible' : 'Not configured'}
            </span>
          </div>
          <dl>
            <dt>Root</dt>
            <dd>{displayValue(workspaceData?.root, 'not configured')}</dd>
            <dt>State</dt>
            <dd>{displayValue(workspaceData?.state ?? workspace?.error, 'not checked yet')}</dd>
          </dl>
          <ul className="checklist">
            {entries.length > 0 ? (
              entries.map((entry) => (
                <li key={`${entry.kind}:${entry.name}`}>
                  {entry.name} <span className="muted-text">{entry.kind}</span>
                </li>
              ))
            ) : (
              <li>
                {workspaceList?.ok ? 'Workspace root is empty' : 'Workspace list unavailable'}
              </li>
            )}
          </ul>
        </article>

        <article className="card wide">
          <div className="card-header">
            <h2>Runtime</h2>
            <span className="pill muted">Local</span>
          </div>
          <pre>{JSON.stringify({ versions, daemon, workspace, workspaceList }, null, 2)}</pre>
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
