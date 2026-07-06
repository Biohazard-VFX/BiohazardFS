import { LoaderCircle, TriangleAlert, WifiOff } from 'lucide-react';

import { cn } from '@/lib/utils';

// A calm status pill summarizing the sync state at a glance. Priority:
// offline > dirty (waiting to sync) > active transfers > idle.
//
// Artist-facing language only (CLAUDE.md): no daemon/endpoint jargon here.
type Props = {
  transferCount: number;
  dirtyCount: number;
  reachable: boolean;
};

export function SyncStatus({ transferCount, dirtyCount, reachable }: Props) {
  if (!reachable) {
    return (
      <Pill tone="offline" icon={<WifiOff className="size-3" />}>
        Offline
      </Pill>
    );
  }
  if (dirtyCount > 0) {
    return (
      <Pill tone="warn" icon={<TriangleAlert className="size-3" />}>
        {`${String(dirtyCount)} waiting to sync`}
      </Pill>
    );
  }
  if (transferCount > 0) {
    return (
      <Pill tone="active" icon={<LoaderCircle className="size-3 animate-spin" />}>
        {`Syncing ${String(transferCount)}`}
      </Pill>
    );
  }
  return <Pill tone="idle">In sync</Pill>;
}

type Tone = 'idle' | 'active' | 'warn' | 'offline';

function Pill({
  tone,
  icon,
  children,
}: {
  tone: Tone;
  icon?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <span
      className={cn(
        'inline-flex h-6 items-center gap-1.5 rounded-full px-2.5 text-xs font-medium',
        tone === 'idle' && 'bg-muted text-muted-foreground',
        // Normal sync activity is calm and neutral — don't cry wolf.
        tone === 'active' && 'bg-foreground/10 text-foreground',
        // Waiting-to-sync is trouble (unsynced local work) → brand alarm.
        tone === 'warn' && 'bg-primary/15 text-primary',
        tone === 'offline' && 'bg-destructive/15 text-destructive',
      )}
    >
      {icon}
      {children}
    </span>
  );
}
