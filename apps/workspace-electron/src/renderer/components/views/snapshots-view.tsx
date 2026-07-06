import { Database } from 'lucide-react';

import { useDaemonFetch } from '@/lib/use-fetch';
import { isStubbed } from '@/lib/daemon-capabilities';
import { type Entry, asString, entryList } from '@/lib/daemon';
import { formatRelativeTime } from '@/lib/format';
import { ScrollArea } from '@/components/ui/scroll-area';
import { ViewEmpty, ViewLoading } from '@/components/view-states';

// Read-only snapshot list (snapshot.list — spine). snapshot.create / mount /
// restore are periphery → no actions offered yet, just the list.
export function SnapshotsView({ refreshNonce }: { refreshNonce: number }) {
  const result = useDaemonFetch('snapshot.list', {}, refreshNonce);
  const snapshots = entryList(result.data, ['snapshots', 'entries', 'items']);

  if (result.loading) return <ViewLoading rows={6} />;
  if (snapshots.length === 0) {
    return (
      <ViewEmpty title="No snapshots" icon={<Database />}>
        {isStubbed('snapshot.create')
          ? 'Snapshots will appear here once the daemon supports creating them (snapshot.create is method_not_implemented).'
          : 'No snapshots have been created yet.'}
      </ViewEmpty>
    );
  }

  return (
    <ScrollArea className="h-full">
      <ul className="divide-border divide-y">
        {snapshots.map((snapshot, index) => (
          <SnapshotRow key={`${String(index)}:${asString(snapshot.id)}`} snapshot={snapshot} />
        ))}
      </ul>
    </ScrollArea>
  );
}

function SnapshotRow({ snapshot }: { snapshot: Entry }) {
  const id = asString(snapshot.id ?? snapshot.name, 'snapshot');
  const scope = asString(snapshot.scope);
  const when = formatRelativeTime(snapshot.created_at ?? snapshot.timestamp);
  return (
    <li className="px-4 py-2.5">
      <div className="flex items-center gap-2">
        <span className="truncate text-sm font-medium" title={id}>
          {id}
        </span>
        {when ? <span className="text-muted-foreground ml-auto text-xs">{when}</span> : null}
      </div>
      {scope ? <p className="text-muted-foreground mt-0.5 text-xs">{scope}</p> : null}
    </li>
  );
}
