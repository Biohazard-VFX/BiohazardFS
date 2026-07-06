import { useState } from 'react';
import { Bug, ChevronDown, Minus, Monitor, Moon, Plus, RotateCcw, Sun } from 'lucide-react';

import { useActions } from '@/app/root';
import { type DaemonSnapshot } from '@/lib/use-daemon';
import { isStubbed, METHOD_NOT_IMPLEMENTED } from '@/lib/daemon-capabilities';
import { asString } from '@/lib/daemon';
import { type ReleaseChannel, useAppInfo, usePrefs, useUpdateStatus } from '@/lib/use-prefs';
import { cn } from '@/lib/utils';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible';
import { Input } from '@/components/ui/input';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Separator } from '@/components/ui/separator';
import { Feedback } from '@/components/view-states';

type Props = {
  snapshot: DaemonSnapshot;
};

export function SettingsView({ snapshot }: Props) {
  const { saveCacheLocation } = useActions();
  const [path, setPath] = useState('');
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<{ ok: boolean; text: string } | null>(null);

  const workspaceRoot = asString(
    (snapshot.workspace?.body as { data?: { root?: string } } | undefined)?.data?.root,
  );
  const endpoint = snapshot.daemon?.endpoint ?? '127.0.0.1:47666';
  const versions = snapshot.versions;

  async function save() {
    const trimmed = path.trim();
    if (!trimmed) {
      setFeedback({ ok: false, text: 'Enter a folder path first.' });
      return;
    }
    setFeedback(null);
    setBusy(true);
    const result = await saveCacheLocation(trimmed);
    setBusy(false);
    const err = (result.body as { error?: { code?: string } } | undefined)?.error;
    if (err?.code === METHOD_NOT_IMPLEMENTED) {
      setFeedback({ ok: false, text: 'Requires daemon support (config.set not built).' });
      return;
    }
    if (err?.code) {
      setFeedback({ ok: false, text: `Could not save (${err.code}).` });
    } else {
      setFeedback({ ok: true, text: 'Cache location saved.' });
      setPath('');
    }
  }

  // config.set is periphery (method_not_implemented). The cache-location field
  // stays visible (read-only value is useful) but Save is gated.
  const canSetConfig = !isStubbed('config.set');

  return (
    <ScrollArea className="h-full">
      <div className="mx-auto flex max-w-2xl flex-col gap-4 p-4">
        <Card className="py-4">
          <CardHeader className="pb-0">
            <CardTitle className="text-sm">Cache location</CardTitle>
            <CardDescription>Where BiohazardFS keeps local copies of your files.</CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-2">
            <form
              className="flex items-center gap-2"
              onSubmit={(e) => {
                e.preventDefault();
                void save();
              }}
            >
              <Input
                value={path}
                onChange={(e) => {
                  setPath(e.target.value);
                }}
                placeholder={workspaceRoot || '/Volumes/Work/biohazardfs-cache'}
                aria-label="Cache location"
                className="font-mono"
              />
              <Button
                type="submit"
                disabled={busy || !canSetConfig}
                title={canSetConfig ? undefined : 'Requires daemon support (config.set not built)'}
              >
                {busy ? 'Saving…' : 'Save'}
              </Button>
            </form>
            {feedback ? <Feedback ok={feedback.ok}>{feedback.text}</Feedback> : null}
          </CardContent>
        </Card>

        <CacheLimitSection />

        <AppearanceSection />

        <WindowSection />

        <Card className="py-4">
          <CardHeader className="pb-0">
            <CardTitle className="text-sm">Daemon</CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-1.5 font-mono text-xs">
            <Row label="Endpoint" value={endpoint} />
            <Row label="Token" value="•••••••• (local only)" />
            {workspaceRoot ? <Row label="Workspace root" value={workspaceRoot} /> : null}
          </CardContent>
        </Card>

        <Card className="py-4">
          <CardHeader className="pb-0">
            <CardTitle className="text-sm">Versions</CardTitle>
          </CardHeader>
          <CardContent className="grid grid-cols-2 gap-1.5 font-mono text-xs">
            <Row label="App" value={versions ? `v${versions.app}` : '—'} />
            <Row label="Electron" value={versions?.electron ?? '—'} />
            <Row label="Chrome" value={versions?.chrome ?? '—'} />
            <Row label="Node" value={versions?.node ?? '—'} />
          </CardContent>
        </Card>

        <UpdatesSection />

        <Separator />

        {/* Advanced diagnostics tucked away from the default artist flow
            (CLAUDE.md). Raw daemon envelopes; field names are draft. */}
        <Collapsible>
          <CollapsibleTrigger asChild>
            <Button variant="ghost" size="sm" className="w-full justify-between">
              <span className="flex items-center gap-2">
                <Bug className="size-3.5" />
                Diagnostics
              </span>
              <ChevronDown className="size-3.5" />
            </Button>
          </CollapsibleTrigger>
          <CollapsibleContent>
            <pre className="bg-muted text-muted-foreground mt-2 max-h-80 overflow-auto rounded-md p-3 text-[0.7rem]">
              {JSON.stringify(
                {
                  versions: snapshot.versions,
                  daemon: snapshot.daemon,
                  workspace: snapshot.workspace,
                  workspaceList: snapshot.workspaceList,
                  cacheStatus: snapshot.cacheStatus,
                  cacheList: snapshot.cacheList,
                  transferList: snapshot.transferList,
                  conflictList: snapshot.conflictList,
                  lockList: snapshot.lockList,
                },
                null,
                2,
              )}
            </pre>
          </CollapsibleContent>
        </Collapsible>
      </div>
    </ScrollArea>
  );
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

const ZOOM_MIN = 0.5;
const ZOOM_MAX = 2;

// Theme (light/dark/system). Persisted as a UI pref alongside zoom and chrome.
// The actual class swap happens in useTheme; this just writes the choice.
function AppearanceSection() {
  const { prefs, setTheme } = usePrefs();
  const theme = prefs?.theme ?? 'dark';
  const options: Array<{ value: 'light' | 'dark' | 'system'; label: string; icon: typeof Sun }> = [
    { value: 'light', label: 'Light', icon: Sun },
    { value: 'dark', label: 'Dark', icon: Moon },
    { value: 'system', label: 'System', icon: Monitor },
  ];
  return (
    <Card className="py-4">
      <CardHeader className="pb-0">
        <CardTitle className="text-sm">Appearance</CardTitle>
        <CardDescription>Interface color scheme. System follows your OS setting.</CardDescription>
      </CardHeader>
      <CardContent>
        <div className="flex gap-1">
          {options.map((option) => {
            const Icon = option.icon;
            const active = theme === option.value;
            return (
              <Button
                key={option.value}
                variant={active ? 'default' : 'outline'}
                size="sm"
                onClick={() => void setTheme(option.value)}
              >
                <Icon className="size-3.5" />
                {option.label}
              </Button>
            );
          })}
        </div>
      </CardContent>
    </Card>
  );
}

// Window chrome + zoom. These are Electron-owned presentation prefs (not daemon
// config): persisted to userData/prefs.json. Zoom applies live; changing the
// chrome requires a restart because the OS frame is fixed at window creation.
// Cache size preference. Lives in Electron prefs until daemon quota support can
// enforce it. null = no local preference.
function CacheLimitSection() {
  const { prefs, setCacheLimit } = usePrefs();
  const limit = prefs?.cacheLimitGB ?? null;
  const [draft, setDraft] = useState(limit !== null ? String(limit) : '');
  const [busy, setBusy] = useState(false);

  async function save() {
    const trimmed = draft.trim();
    setBusy(true);
    await setCacheLimit(trimmed === '' ? null : Number(trimmed));
    setBusy(false);
  }

  return (
    <Card className="py-4">
      <CardHeader className="pb-0">
        <CardTitle className="text-sm">Cache size preference</CardTitle>
        <CardDescription>
          Preferred local cache size in GB. Saved locally for future daemon quota support; not
          enforced yet.
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-2">
        <form
          className="flex items-center gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            void save();
          }}
        >
          <Input
            type="number"
            min={1}
            value={draft}
            onChange={(e) => {
              setDraft(e.target.value);
            }}
            placeholder="No limit"
            aria-label="Cache limit in GB"
            className="w-32"
          />
          <Button type="submit" size="sm" disabled={busy}>
            {busy ? 'Saving…' : 'Save'}
          </Button>
          {limit !== null ? (
            <span className="text-muted-foreground text-xs">Current: {String(limit)} GB</span>
          ) : (
            <span className="text-muted-foreground text-xs">Current: no limit</span>
          )}
        </form>
      </CardContent>
    </Card>
  );
}

function UpdatesSection() {
  const { prefs, setReleaseChannel, setAutoUpdateChecks } = usePrefs();
  const { status, checking, checkNow, refreshStatus } = useUpdateStatus();
  const channel = prefs?.releaseChannel ?? 'dev';
  const autoChecks = prefs?.autoUpdateChecks ?? false;
  const channels: ReleaseChannel[] = ['dev', 'nightly', 'alpha', 'beta', 'stable'];

  async function changeChannel(next: ReleaseChannel) {
    await setReleaseChannel(next);
    await refreshStatus();
  }

  return (
    <Card className="py-4">
      <CardHeader className="pb-0">
        <CardTitle className="text-sm">Updates</CardTitle>
        <CardDescription>
          Installer update checks are packaged-build only. Downloads remain manual until daemon
          restart safety is implemented.
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <div className="flex flex-wrap gap-1">
          {channels.map((option) => (
            <Button
              key={option}
              variant={channel === option ? 'default' : 'outline'}
              size="sm"
              onClick={() => void changeChannel(option)}
            >
              {option}
            </Button>
          ))}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Button
            size="sm"
            variant={autoChecks ? 'default' : 'outline'}
            onClick={() => void setAutoUpdateChecks(!autoChecks)}
          >
            {autoChecks ? 'Auto-check enabled' : 'Auto-check disabled'}
          </Button>
          <Button size="sm" variant="outline" disabled={checking} onClick={() => void checkNow()}>
            <RotateCcw className={cn('size-3.5', checking && 'animate-spin')} />
            {checking ? 'Checking…' : 'Check now'}
          </Button>
        </div>
        <div className="bg-muted/40 rounded-md border px-3 py-2 font-mono text-xs">
          <Row label="Channel" value={status?.channel ?? channel} />
          <Row label="State" value={status?.state ?? 'idle'} />
          <Row label="Packaged" value={status?.packaged ? 'yes' : 'no'} />
          {status?.updateVersion ? <Row label="Update" value={status.updateVersion} /> : null}
          {status?.checkedAt ? <Row label="Checked" value={status.checkedAt} /> : null}
        </div>
        {status?.message ? (
          <Feedback ok={status.state !== 'error'}>{status.message}</Feedback>
        ) : null}
      </CardContent>
    </Card>
  );
}

function WindowSection() {
  const { prefs, setZoom, setWindowChrome } = usePrefs();
  const appInfo = useAppInfo();
  const platform = appInfo?.platform;
  const nativeLabel =
    platform === 'darwin' || platform === 'win32' ? 'native (macOS/Windows)' : 'native';

  const zoom = prefs?.zoomFactor ?? 1;
  const zoomPct = Math.round(zoom * 100);

  return (
    <Card className="py-4">
      <CardHeader className="pb-0">
        <CardTitle className="text-sm">Window</CardTitle>
        <CardDescription>
          Window decorations and interface scaling. Scaling also responds to Ctrl + / − / 0.
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <div>
          <p className="text-muted-foreground mb-1.5 text-[0.62rem] tracking-widest uppercase">
            Decorations
          </p>
          <div className="flex gap-1">
            {(['auto', 'native', 'frameless'] as const).map((option) => {
              const active = (prefs?.windowChrome ?? 'auto') === option;
              const label =
                option === 'auto'
                  ? `Auto (${platform === 'linux' ? 'frameless' : 'native'})`
                  : option === 'native'
                    ? nativeLabel
                    : 'Frameless';
              return (
                <Button
                  key={option}
                  variant={active ? 'default' : 'outline'}
                  size="sm"
                  onClick={() => void setWindowChrome(option)}
                >
                  {label}
                </Button>
              );
            })}
          </div>
          <p className="text-muted-foreground mt-1.5 text-xs">
            Restart the app to apply decoration changes.
          </p>
        </div>

        <Separator />

        <div className="flex items-center gap-2">
          <p className="text-muted-foreground flex-1 text-[0.62rem] tracking-widest uppercase">
            Scaling
          </p>
          <div className="flex items-center gap-1">
            <Button
              variant="outline"
              size="icon"
              aria-label="Zoom out"
              disabled={zoom <= ZOOM_MIN}
              onClick={() => void setZoom(zoom - 0.1)}
            >
              <Minus className="size-3.5" />
            </Button>
            <span className="text-muted-foreground w-12 text-center font-mono text-xs">
              {`${String(zoomPct)}%`}
            </span>
            <Button
              variant="outline"
              size="icon"
              aria-label="Zoom in"
              disabled={zoom >= ZOOM_MAX}
              onClick={() => void setZoom(zoom + 0.1)}
            >
              <Plus className="size-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="sm"
              aria-label="Reset scaling"
              disabled={zoom === 1}
              onClick={() => void setZoom(1)}
              className={cn('ml-1')}
            >
              <RotateCcw className="size-3.5" />
              Reset
            </Button>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}
