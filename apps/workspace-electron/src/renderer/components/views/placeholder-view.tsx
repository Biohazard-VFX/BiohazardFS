import { Construction } from 'lucide-react';

import { VIEW_TITLES } from '@/app/nav';

// Honest "reserved" surface for views whose IA is in place but whose content
// lands in a later phase (Locks, Audit, Snapshots, Access) or which the daemon
// doesn't back yet (Members/Devices/Permissions/Storage). Never pretends to work.
export function PlaceholderView({ view, note }: { view: keyof typeof VIEW_TITLES; note: string }) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-2 p-10 text-center">
      <Construction className="text-muted-foreground/50 size-7" />
      <p className="text-sm font-medium">{VIEW_TITLES[view]}</p>
      <p className="text-muted-foreground max-w-sm text-xs">{note}</p>
    </div>
  );
}
