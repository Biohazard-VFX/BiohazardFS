import { useState } from 'react';
import { ExternalLink, RefreshCw } from 'lucide-react';

import { type DaemonSnapshot } from '@/lib/use-daemon';
import { useDaemonFetch } from '@/lib/use-fetch';
import { isStubbed } from '@/lib/daemon-capabilities';
import { extractData, mountAttached, mountPathFromList } from '@/lib/daemon';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { ScrollArea } from '@/components/ui/scroll-area';

// Studio connection & mount state (DASHBOARD_UX §5.1/§5.2). Real data from
// auth.whoami / auth.status / mount.status / mount.list (all spine). Mount
// attach/detach/repair are periphery → gated as "requires daemon support".
type Props = {
  snapshot: DaemonSnapshot;
  refreshNonce: number;
};

export function ConnectionView({ snapshot, refreshNonce }: Props) {
  const whoami = useDaemonFetch('auth.whoami', {}, refreshNonce);
  const authStatus = useDaemonFetch('auth.status', {}, refreshNonce);
  const endpoint = snapshot.daemon?.endpoint ?? '127.0.0.1:47666';
  const mountStatusData = extractData(snapshot.mountStatus);
  const mountListData = extractData(snapshot.mountList);

  const [openFeedback, setOpenFeedback] = useState<string | null>(null);

  const user = field(whoami.data, ['user', 'display_name', 'name', 'email']);
  const deviceId = field(authStatus.data, ['device_id', 'device']);
  const authState = field(authStatus.data, ['state', 'status']);
  const mountPath = mountPathFromList(mountListData);
  const mounted = mountAttached(mountStatusData);

  const mountOpsAvailable = !isStubbed('mount.attach');

  async function openInFileManager() {
    if (!mountPath) return;
    setOpenFeedback(null);
    const res = await window.biohazardfs.openPath(mountPath);
    if (!res.ok) setOpenFeedback(res.error ?? 'Could not open folder.');
  }

  return (
    <ScrollArea className="h-full">
      <div className="mx-auto flex max-w-2xl flex-col gap-4 p-4">
        <Card className="py-4">
          <CardHeader className="pb-0">
            <CardTitle className="text-sm">Account</CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-1.5 font-mono text-xs">
            <Row label="Signed in" value={user ?? (whoami.loading ? '…' : 'unknown')} />
            <Row label="Auth state" value={authState ?? (authStatus.loading ? '…' : 'unknown')} />
            <Row label="Device" value={deviceId ?? (authStatus.loading ? '…' : 'unknown')} />
          </CardContent>
        </Card>

        <Card className="py-4">
          <CardHeader className="pb-0">
            <CardTitle className="text-sm">Server</CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-1.5 font-mono text-xs">
            <Row label="Endpoint" value={endpoint} />
            <Row
              label="Reachable"
              value={snapshot.daemon?.body !== undefined ? 'yes' : 'no (showing last known)'}
            />
          </CardContent>
        </Card>

        <Card className="py-4">
          <CardHeader className="pb-0">
            <CardTitle className="text-sm">Mount</CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-2">
            <div className="flex flex-col gap-1.5 font-mono text-xs">
              <Row
                label="Path"
                value={mountPath || (snapshot.mountStatus === null ? '…' : 'not mounted')}
              />
              <Row label="State" value={mounted ? 'mounted' : 'unmounted'} />
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <Button
                size="sm"
                variant="outline"
                disabled={!mounted || !mountPath}
                onClick={() => void openInFileManager()}
              >
                <ExternalLink className="size-3.5" />
                Open in file manager
              </Button>
              <Button
                size="sm"
                variant="ghost"
                disabled={!mountOpsAvailable}
                title={
                  mountOpsAvailable
                    ? undefined
                    : 'Requires daemon support (mount.attach / detach not built)'
                }
              >
                <RefreshCw className="size-3.5" />
                Remount
              </Button>
            </div>
            {openFeedback ? <p className="text-primary text-xs">{openFeedback}</p> : null}
            {!mountOpsAvailable ? (
              <p className="text-muted-foreground text-xs">
                Remount / repair / unmount require daemon support (mount.attach · mount.detach ·
                mount.repair return method_not_implemented).
              </p>
            ) : null}
          </CardContent>
        </Card>

        {mountListData ? (
          <Card className="py-4">
            <CardHeader className="pb-0">
              <CardTitle className="text-sm">Mounts</CardTitle>
            </CardHeader>
            <CardContent>
              <pre className="text-muted-foreground max-h-60 overflow-auto rounded-md bg-muted/40 p-3 text-[0.7rem]">
                {JSON.stringify(mountListData, null, 2)}
              </pre>
            </CardContent>
          </Card>
        ) : null}
      </div>
    </ScrollArea>
  );
}

function field(data: Record<string, unknown> | null, keys: string[]): string | null {
  for (const k of keys) {
    const v = data?.[k];
    if (typeof v === 'string' && v.length > 0) return v;
  }
  return null;
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-center justify-between gap-3">
      <span className="text-muted-foreground tracking-widest uppercase text-[0.62rem]">
        {label}
      </span>
      <span className="truncate">{value}</span>
    </div>
  );
}
