import { ChevronDown, HardDrive } from 'lucide-react';
import { useState } from 'react';

import { ADMIN_ITEMS, MAIN_ITEMS, type NavItem, type ViewId } from '@/app/nav';
import { type DaemonCounts } from '@/lib/derive';
import { formatBytes } from '@/lib/format';
import { cn } from '@/lib/utils';
import { Badge } from '@/components/ui/badge';
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible';
import { Progress } from '@/components/ui/progress';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Separator } from '@/components/ui/separator';

type Props = {
  view: ViewId;
  onViewChange: (view: ViewId) => void;
  counts: DaemonCounts;
  studioLabel: string;
  workspaceReady: boolean;
  reachable: boolean;
};

export function AppSidebar({
  view,
  onViewChange,
  counts,
  studioLabel,
  workspaceReady,
  reachable,
}: Props) {
  const [adminOpen, setAdminOpen] = useState(false);
  return (
    <aside className="bg-sidebar text-sidebar-foreground flex h-full w-60 shrink-0 flex-col border-r">
      {/* Selected-studio header. */}
      <div className="flex h-14 items-center gap-2.5 px-4">
        <span className="flex min-w-0 flex-col leading-tight">
          <span className="truncate text-sm font-semibold tracking-tight">{studioLabel}</span>
          <span
            className={cn(
              'text-[0.65rem] font-medium tracking-wide',
              reachable ? 'text-muted-foreground' : 'text-destructive',
            )}
          >
            {reachable ? (workspaceReady ? 'Mounted · online' : 'Connected · no mount') : 'Offline'}
          </span>
        </span>
      </div>

      <Separator />

      <ScrollArea className="flex-1 px-2">
        <nav className="flex flex-col gap-0.5 pt-2">
          {MAIN_ITEMS.map((item) => (
            <NavButton
              key={item.id}
              item={item}
              active={view === item.id}
              counts={counts}
              onViewChange={onViewChange}
            />
          ))}
        </nav>

        <Collapsible open={adminOpen} onOpenChange={setAdminOpen} className="mt-3">
          <CollapsibleTrigger className="text-muted-foreground hover:text-foreground flex w-full items-center gap-1.5 px-2.5 py-1.5 text-[0.62rem] font-semibold tracking-widest uppercase">
            Admin
            <ChevronDown
              className={cn('size-3 transition-transform', adminOpen ? '' : '-rotate-90')}
            />
          </CollapsibleTrigger>
          <CollapsibleContent>
            <nav className="flex flex-col gap-0.5 pb-2">
              {ADMIN_ITEMS.map((item) => (
                <NavButton
                  key={item.id}
                  item={item}
                  active={view === item.id}
                  counts={counts}
                  onViewChange={onViewChange}
                />
              ))}
            </nav>
          </CollapsibleContent>
        </Collapsible>
      </ScrollArea>

      <Separator />
      <StorageFooter counts={counts} />
    </aside>
  );
}

function NavButton({
  item,
  active,
  counts,
  onViewChange,
}: {
  item: NavItem;
  active: boolean;
  counts: DaemonCounts;
  onViewChange: (view: ViewId) => void;
}) {
  const Icon = item.icon;
  const count = badgeValue(item.badgeKind, counts);
  const alert = item.badgeKind === 'dirty' || item.badgeKind === 'conflicts';
  return (
    <button
      key={item.id}
      type="button"
      disabled={item.stubbed}
      title={item.stubbed ? `Requires daemon support (${item.label} not built)` : undefined}
      aria-current={active ? 'page' : undefined}
      onClick={() => {
        if (!item.stubbed) onViewChange(item.id);
      }}
      className={cn(
        'relative flex h-8 items-center gap-2.5 rounded-md px-2.5 text-sm transition-colors',
        item.stubbed && 'cursor-not-allowed opacity-40',
        !item.stubbed &&
          (active
            ? 'bg-sidebar-accent text-sidebar-accent-foreground font-medium'
            : 'text-muted-foreground hover:bg-sidebar-accent/60 hover:text-sidebar-accent-foreground'),
      )}
    >
      {active ? (
        <span
          aria-hidden="true"
          className="bg-primary absolute top-1.5 bottom-1.5 left-0 w-0.5 rounded-full"
        />
      ) : null}
      <Icon className={cn('size-4 shrink-0', active && 'text-primary')} />
      <span className="flex-1 text-left">{item.label}</span>
      {count > 0 ? (
        <Badge
          variant={alert ? 'secondary' : 'outline'}
          className={cn(
            'h-5 px-1.5 text-[0.65rem] tabular-nums',
            !alert && 'border-transparent bg-foreground/10 text-foreground',
          )}
        >
          {count}
        </Badge>
      ) : null}
    </button>
  );
}

function badgeValue(kind: NavItem['badgeKind'] | undefined, counts: DaemonCounts): number {
  switch (kind) {
    case 'transfers':
      return counts.transferCount;
    case 'dirty':
      return counts.dirtyCount;
    case 'conflicts':
      return counts.conflictCount;
    case 'locks':
      return counts.lockCount;
    default:
      return 0;
  }
}

function StorageFooter({ counts }: { counts: DaemonCounts }) {
  const used = counts.usedBytes;
  const quota = counts.quotaBytes;
  const hasQuota = used !== null && quota !== null && quota > 0;
  const ratio = hasQuota ? Math.min(used / quota, 1) : 0;
  const dirty = counts.dirtyBytes;
  return (
    <div className="px-3 py-3">
      <div className="text-muted-foreground mb-2 flex items-center gap-2 text-xs">
        <HardDrive className="size-3.5" />
        <span>Local cache</span>
      </div>
      {used !== null ? (
        <p className="mb-1.5 text-sm font-medium">
          {formatBytes(used)}
          {hasQuota ? <span className="text-muted-foreground"> / {formatBytes(quota)}</span> : null}
        </p>
      ) : (
        <p className="text-muted-foreground mb-1.5 text-sm">unknown usage</p>
      )}
      {hasQuota ? <Progress value={Math.round(ratio * 100)} className="h-1.5" /> : null}
      {dirty !== null && dirty > 0 ? (
        <p className="text-primary mt-2 text-xs font-medium">
          {formatBytes(dirty)} waiting to sync
        </p>
      ) : null}
    </div>
  );
}
