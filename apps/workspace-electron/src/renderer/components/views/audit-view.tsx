import { Scroll } from 'lucide-react';

import { useDaemonFetch } from '@/lib/use-fetch';
import { type Entry, asString, entryList } from '@/lib/daemon';
import { formatRelativeTime } from '@/lib/format';
import { ScrollArea } from '@/components/ui/scroll-area';
import { ViewEmpty, ViewLoading } from '@/components/view-states';

// Read-only audit log (audit.events — spine). Field names are draft; render
// defensively and fall back to a couple of common keys.
export function AuditView({ refreshNonce }: { refreshNonce: number }) {
  const result = useDaemonFetch('audit.events', {}, refreshNonce);
  const events = entryList(result.data, ['events', 'entries', 'items']);

  if (result.loading) return <ViewLoading rows={6} />;
  if (events.length === 0) {
    return (
      <ViewEmpty title="No audit events" icon={<Scroll />}>
        Provenance events (UI / CLI / agent / API actions) will be recorded here once the daemon
        emits them.
      </ViewEmpty>
    );
  }

  return (
    <ScrollArea className="h-full">
      <ul className="divide-border divide-y">
        {events.map((event, index) => (
          <AuditRow key={`${String(index)}:${asString(event.id)}`} event={event} />
        ))}
      </ul>
    </ScrollArea>
  );
}

function AuditRow({ event }: { event: Entry }) {
  const action = asString(event.event_type ?? event.action ?? event.method);
  const actor = asString(event.actor ?? event.actor_id ?? event.user);
  const target = asString(event.target ?? event.path ?? event.node_id);
  const when = formatRelativeTime(event.timestamp ?? event.created_at);
  return (
    <li className="px-4 py-2">
      <div className="flex items-center gap-2">
        <span className="text-sm font-medium">{action || 'event'}</span>
        {when ? <span className="text-muted-foreground ml-auto text-xs">{when}</span> : null}
      </div>
      <p className="text-muted-foreground mt-0.5 text-xs">
        {actor ? `by ${actor}` : null}
        {target ? (actor ? ' · ' : '') + target : null}
      </p>
    </li>
  );
}
