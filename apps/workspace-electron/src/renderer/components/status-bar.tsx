import { useEffect, useState } from 'react';

import { asString } from '@/lib/daemon';

// Bottom strip. Technical details (endpoint, versions, last refresh) live here
// and in Settings, never in the artist-facing views.
type Props = {
  endpoint: string | null;
  reachable: boolean;
  appVersion: string | null;
  lastUpdated: number | null;
};

export function StatusBar({ endpoint, reachable, appVersion, lastUpdated }: Props) {
  return (
    <footer className="text-muted-foreground flex h-6 shrink-0 items-center gap-3 border-t px-3 text-[0.68rem]">
      <span className="inline-flex items-center gap-1.5">
        <span
          className={
            reachable
              ? 'inline-block size-1.5 rounded-full bg-emerald-500'
              : 'inline-block size-1.5 rounded-full bg-destructive'
          }
        />
        {reachable ? 'Daemon connected' : 'Daemon unreachable'}
      </span>
      {endpoint ? <span className="truncate">{asString(endpoint)}</span> : null}
      <div className="flex-1" />
      {appVersion ? <span>{`v${appVersion}`}</span> : null}
      {lastUpdated ? <LastUpdated at={lastUpdated} /> : null}
    </footer>
  );
}

function LastUpdated({ at }: { at: number }) {
  const [, force] = useState(0);
  useEffect(() => {
    const t = setInterval(() => {
      force((n) => n + 1);
    }, 30000);
    return () => {
      clearInterval(t);
    };
  }, []);
  const secs = Math.max(0, Math.round((Date.now() - at) / 1000));
  const label = secs < 60 ? `${String(secs)}s ago` : `${String(Math.round(secs / 60))}m ago`;
  return <span>{`updated ${label}`}</span>;
}
