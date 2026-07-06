import { useState } from 'react';
import { CheckCircle2, GitMerge } from 'lucide-react';

import { useActions } from '@/app/root';
import { type DaemonSnapshot } from '@/lib/use-daemon';
import { isStubbed, METHOD_NOT_IMPLEMENTED } from '@/lib/daemon-capabilities';
import { asString, entryList, extractData, extractError } from '@/lib/daemon';
import { formatRelativeTime } from '@/lib/format';
import { Button } from '@/components/ui/button';
import { ScrollArea } from '@/components/ui/scroll-area';
import { ViewEmpty, ViewError, ViewLoading, Feedback } from '@/components/view-states';

type Props = {
  snapshot: DaemonSnapshot;
  loaded: boolean;
  query: string;
};

export function ConflictsView({ snapshot, loaded, query }: Props) {
  const { preserveAllConflicts } = useActions();
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<{ ok: boolean; text: string } | null>(null);

  const data = extractData(snapshot.conflictList);
  const error = extractError(snapshot.conflictList);
  const conflicts = entryList(data, ['entries', 'conflicts', 'items']);
  const filtered = query
    ? conflicts.filter((c) => asString(c.path).toLowerCase().includes(query.toLowerCase()))
    : conflicts;

  async function preserveAll() {
    setFeedback(null);
    setBusy(true);
    const result = await preserveAllConflicts();
    setBusy(false);
    const err = extractError(result);
    if (err?.code === METHOD_NOT_IMPLEMENTED) {
      setFeedback({
        ok: false,
        text: 'Requires daemon support (conflict.preserve_all not built).',
      });
      return;
    }
    setFeedback(
      err
        ? { ok: false, text: `Preserve all failed (${err.code}).` }
        : { ok: true, text: 'All versions preserved.' },
    );
  }

  // conflict.preserve_all/resolve are periphery. Gate proactively.
  const preserveAvailable = !isStubbed('conflict.preserve_all');
  const preserveTitle = preserveAvailable
    ? undefined
    : 'Requires daemon support (conflict.preserve_all not built)';

  if (!loaded && snapshot.conflictList === null) {
    return <ViewLoading rows={3} />;
  }
  if (error && conflicts.length === 0) {
    return <ViewError label="Conflict list unavailable" error={error} />;
  }

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-10 shrink-0 items-center gap-2 border-b px-3">
        <Button
          size="sm"
          disabled={busy || conflicts.length === 0 || !preserveAvailable}
          title={preserveTitle}
          onClick={() => void preserveAll()}
        >
          <GitMerge className="size-3.5" />
          {busy ? 'Preserving…' : 'Preserve all versions'}
        </Button>
        {feedback ? <Feedback ok={feedback.ok}>{feedback.text}</Feedback> : null}
      </div>

      <ScrollArea className="min-h-0 flex-1">
        {filtered.length === 0 ? (
          <ViewEmpty title={query ? 'No matches' : 'No conflicts'} icon={<CheckCircle2 />}>
            {query
              ? `Nothing here matches “${query}”.`
              : 'Divergent work is always preserved separately. Conflicts will be listed here when they happen.'}
          </ViewEmpty>
        ) : (
          <ul className="divide-border divide-y">
            {filtered.map((conflict, index) => {
              const id = asString(conflict.id ?? conflict.path, `conflict-${String(index)}`);
              const path = asString(conflict.path, 'unknown path');
              const actor = asString(conflict.actor);
              const when = formatRelativeTime(conflict.timestamp ?? conflict.created_at);
              return (
                <li key={`${String(index)}:${id}`} className="px-4 py-2.5">
                  <div className="flex items-center gap-2">
                    <span className="min-w-0 flex-1 truncate text-sm font-medium" title={path}>
                      {path}
                    </span>
                    {conflict.kind ? (
                      <span className="text-muted-foreground text-xs">
                        {asString(conflict.kind)}
                      </span>
                    ) : null}
                  </div>
                  <div className="text-muted-foreground mt-0.5 flex items-center gap-2 text-xs">
                    {actor ? <span>{`by ${actor}`}</span> : null}
                    {when ? <span>{when}</span> : null}
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </ScrollArea>
    </div>
  );
}
