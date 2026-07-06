import { useState } from 'react';
import { Lock } from 'lucide-react';

import { useDaemonFetch } from '@/lib/use-fetch';
import { isStubbed, METHOD_NOT_IMPLEMENTED } from '@/lib/daemon-capabilities';
import { type Entry, asString, entryList, extractError } from '@/lib/daemon';
import { formatRelativeTime } from '@/lib/format';
import { cn } from '@/lib/utils';
import { Button } from '@/components/ui/button';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import { ViewEmpty, ViewLoading } from '@/components/view-states';

// Lock management (DASHBOARD_UX §5.6). lock.list / lock.acquire / lock.release /
// lock.extend are spine (real); lock.break is periphery → gated. The view owns
// its lock.list fetch (not in the global snapshot) and re-fetches after each
// action via a local nonce.
type Props = {
  refreshNonce: number;
};

export function LocksView({ refreshNonce }: Props) {
  const [actionNonce, setActionNonce] = useState(0);
  const [busy, setBusy] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<{ ok: boolean; text: string } | null>(null);

  const result = useDaemonFetch('lock.list', {}, refreshNonce + actionNonce);
  const locks = entryList(result.data, ['entries', 'locks', 'items']);

  async function act(method: 'lock.release' | 'lock.extend', path: string) {
    if (isStubbed(method)) {
      setFeedback({ ok: false, text: `Requires daemon support (${method} not built).` });
      return;
    }
    setFeedback(null);
    setBusy(path);
    const res = await window.biohazardfs.rpc(method, { path });
    setBusy(null);
    const err = extractError(res);
    if (err?.code === METHOD_NOT_IMPLEMENTED) {
      setFeedback({ ok: false, text: `Requires daemon support (${method} not built).` });
    } else if (err) {
      setFeedback({ ok: false, text: `${method} failed (${err.code}).` });
    } else {
      setFeedback({
        ok: true,
        text: `${method === 'lock.release' ? 'Released' : 'Extended'} ${path}.`,
      });
      setActionNonce((n) => n + 1);
    }
  }

  if (result.loading) return <ViewLoading rows={4} />;

  const breakAvailable = !isStubbed('lock.break');

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-10 shrink-0 items-center gap-2 border-b px-3">
        <span className="text-muted-foreground text-xs">
          {locks.length > 0 ? `${String(locks.length)} lock(s) held` : 'No locks held'}
        </span>
        {feedback ? (
          <span className={cn('text-xs', feedback.ok ? 'text-muted-foreground' : 'text-primary')}>
            {feedback.text}
          </span>
        ) : null}
      </div>
      <ScrollArea className="min-h-0 flex-1">
        {locks.length === 0 ? (
          <ViewEmpty title="No locks held" icon={<Lock />}>
            Scene and binary files can be locked to block conflicting edits. Locks you acquire will
            appear here.
          </ViewEmpty>
        ) : (
          <ul className="divide-border divide-y">
            {locks.map((lock, index) => (
              <LockRow
                key={`${String(index)}:${asString(lock.path)}`}
                lock={lock}
                busy={busy}
                onAct={(method, path) => {
                  void act(method, path);
                }}
                breakAvailable={breakAvailable}
              />
            ))}
          </ul>
        )}
      </ScrollArea>
    </div>
  );
}

function LockRow({
  lock,
  busy,
  onAct,
  breakAvailable,
}: {
  lock: Entry;
  busy: string | null;
  onAct: (method: 'lock.release' | 'lock.extend', path: string) => void;
  breakAvailable: boolean;
}) {
  const path = asString(lock.path, 'unknown path');
  const owner = asString(lock.owner);
  const expires = formatRelativeTime(lock.expires ?? lock.expires_at);
  // Default to locked when state unknown — never falsely advertise "safe to edit."
  const locked = lock.locked !== false && asString(lock.state) !== 'unlocked';
  return (
    <li className="flex items-center gap-3 px-4 py-2.5">
      <Lock className={cn('size-4 shrink-0', locked ? 'text-primary' : 'text-muted-foreground')} />
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-medium" title={path}>
          {path}
        </p>
        <p className="text-muted-foreground text-xs">
          {owner ? `by ${owner}` : 'owner unknown'}
          {expires ? ` · expires ${expires}` : ''}
        </p>
      </div>
      <Button
        variant="ghost"
        size="sm"
        disabled={busy === path}
        onClick={() => {
          onAct('lock.extend', path);
        }}
      >
        Extend
      </Button>
      <Button
        variant="ghost"
        size="sm"
        disabled={busy === path}
        onClick={() => {
          onAct('lock.release', path);
        }}
      >
        Release
      </Button>
      <Tooltip>
        <TooltipTrigger asChild>
          <span>
            <Button variant="ghost" size="sm" disabled={!breakAvailable}>
              Break
            </Button>
          </span>
        </TooltipTrigger>
        <TooltipContent>
          {breakAvailable
            ? 'Force-break this lock'
            : 'Requires daemon support (lock.break not built)'}
        </TooltipContent>
      </Tooltip>
    </li>
  );
}
