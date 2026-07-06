import { useState } from 'react';
import {
  ArrowDownToLine,
  ArrowUpFromLine,
  CheckCircle2,
  Pause,
  Play,
  TriangleAlert,
} from 'lucide-react';

import { useActions } from '@/app/root';
import { type DaemonSnapshot } from '@/lib/use-daemon';
import { isStubbed, METHOD_NOT_IMPLEMENTED } from '@/lib/daemon-capabilities';
import {
  type Entry,
  asString,
  computeProgress,
  directionLabel,
  entryList,
  extractData,
  extractError,
  stateLabel,
} from '@/lib/daemon';
import { formatBytes } from '@/lib/format';
import { cn } from '@/lib/utils';
import { Button } from '@/components/ui/button';
import { Progress } from '@/components/ui/progress';
import { ScrollArea } from '@/components/ui/scroll-area';
import { ViewEmpty, ViewError, ViewLoading, Feedback } from '@/components/view-states';

type Props = {
  snapshot: DaemonSnapshot;
  loaded: boolean;
};

// Transfers, grouped by outcome state (DASHBOARD_UX §5.4): Uploading /
// Downloading / Queued / Failed / Completed. Pause/resume gated (periphery).
export function ActivityView({ snapshot, loaded }: Props) {
  const { pauseTransfers, resumeTransfers } = useActions();
  const [busy, setBusy] = useState<'pause' | 'resume' | null>(null);
  const [feedback, setFeedback] = useState<{ ok: boolean; text: string } | null>(null);

  const data = extractData(snapshot.transferList);
  const error = extractError(snapshot.transferList);
  const transfers = entryList(data, ['entries', 'transfers', 'items']);

  const groups: {
    title: string;
    icon: typeof ArrowUpFromLine;
    items: Entry[];
    danger?: boolean;
  }[] = [
    { title: 'Uploading', icon: ArrowUpFromLine, items: transfers.filter(isUploading) },
    { title: 'Downloading', icon: ArrowDownToLine, items: transfers.filter(isDownloading) },
    { title: 'Queued', icon: ArrowUpFromLine, items: transfers.filter(isQueued) },
    { title: 'Failed', icon: TriangleAlert, items: transfers.filter(isFailed), danger: true },
    { title: 'Completed', icon: CheckCircle2, items: transfers.filter(isCompleted) },
  ];
  const visible = groups.filter((g) => g.items.length > 0);
  const totalActive = transfers.filter(isActive).length;

  async function run(kind: 'pause' | 'resume') {
    setFeedback(null);
    setBusy(kind);
    const result = kind === 'pause' ? await pauseTransfers() : await resumeTransfers();
    setBusy(null);
    const err = extractError(result);
    if (err?.code === METHOD_NOT_IMPLEMENTED) {
      setFeedback({
        ok: false,
        text: 'Requires daemon support (transfer pause/resume not built).',
      });
      return;
    }
    setFeedback(
      err
        ? { ok: false, text: `${kind === 'pause' ? 'Pause' : 'Resume'} failed (${err.code}).` }
        : { ok: true, text: kind === 'pause' ? 'Transfers paused.' : 'Transfers resumed.' },
    );
  }

  const transfersControllable = !isStubbed('transfer.pause') && !isStubbed('transfer.resume');
  const controlTitle = transfersControllable
    ? undefined
    : 'Requires daemon support (transfer pause/resume not built)';

  if (!loaded && snapshot.transferList === null) {
    return <ViewLoading rows={4} />;
  }
  if (error && transfers.length === 0) {
    return <ViewError label="Transfer list unavailable" error={error} />;
  }

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-10 shrink-0 items-center gap-2 border-b px-3">
        <span className="text-muted-foreground text-xs">
          {totalActive > 0 ? `${String(totalActive)} active` : 'Idle'}
        </span>
        <Button
          variant="outline"
          size="sm"
          disabled={busy !== null || !transfersControllable}
          title={controlTitle}
          onClick={() => void run('pause')}
        >
          <Pause className="size-3.5" />
          Pause all
        </Button>
        <Button
          variant="outline"
          size="sm"
          disabled={busy !== null || !transfersControllable}
          title={controlTitle}
          onClick={() => void run('resume')}
        >
          <Play className="size-3.5" />
          Resume all
        </Button>
        {feedback ? <Feedback ok={feedback.ok}>{feedback.text}</Feedback> : null}
      </div>

      <ScrollArea className="min-h-0 flex-1">
        {visible.length === 0 ? (
          <ViewEmpty title="Nothing transferring right now" icon={<CheckCircle2 />}>
            Uploads and downloads will appear here while they run.
          </ViewEmpty>
        ) : (
          <div className="flex flex-col">
            {visible.map((group) => (
              <section key={group.title}>
                <h2 className="text-muted-foreground flex items-center gap-1.5 px-4 py-1.5 text-[0.62rem] font-semibold tracking-widest uppercase">
                  {group.title}
                  <span className="tabular-nums">{String(group.items.length)}</span>
                </h2>
                <ul className="divide-border divide-y border-t">
                  {group.items.map((transfer, index) => (
                    <TransferRow
                      key={`${group.title}:${String(index)}:${asString(transfer.id ?? transfer.transfer_id)}`}
                      transfer={transfer}
                    />
                  ))}
                </ul>
              </section>
            ))}
          </div>
        )}
      </ScrollArea>
    </div>
  );
}

function isUploading(e: Entry): boolean {
  return asString(e.direction) === 'upload' && isActive(e);
}
function isDownloading(e: Entry): boolean {
  return asString(e.direction) === 'download' && isActive(e);
}
function isQueued(e: Entry): boolean {
  const s = asString(e.state);
  return s === 'queued' || s === 'pending';
}
function isFailed(e: Entry): boolean {
  const s = asString(e.state);
  return s === 'failed' || s === 'error';
}
function isCompleted(e: Entry): boolean {
  const s = asString(e.state);
  return s === 'completed' || s === 'done' || s === 'synced';
}
function isActive(entry: Entry): boolean {
  const state = asString(entry.state);
  return (
    state === 'running' ||
    state === 'uploading' ||
    state === 'downloading' ||
    (state !== 'completed' &&
      state !== 'done' &&
      state !== 'synced' &&
      state !== 'failed' &&
      state !== 'error' &&
      state !== 'queued' &&
      state !== 'pending' &&
      state !== '')
  );
}

function TransferRow({ transfer }: { transfer: Entry }) {
  const direction = directionLabel(transfer.direction);
  const state = stateLabel(transfer.state);
  const path = asString(transfer.path, 'unknown path');
  const progress = computeProgress(transfer);
  const done = transfer.bytes_done;
  const total = transfer.bytes_total;
  const up = transfer.direction === 'upload';

  return (
    <li className="px-4 py-2.5">
      <div className="mb-1.5 flex items-center gap-2">
        {up ? (
          <ArrowUpFromLine className="text-muted-foreground size-3.5" />
        ) : (
          <ArrowDownToLine className="text-muted-foreground size-3.5" />
        )}
        <span className="min-w-0 flex-1 truncate text-sm font-medium" title={path}>
          {path}
        </span>
        <span className="text-muted-foreground text-xs">{direction}</span>
        <span
          className={cn(
            'text-xs font-medium',
            state === 'Failed' ? 'text-primary' : 'text-muted-foreground',
          )}
        >
          {state}
        </span>
      </div>
      <div className="flex items-center gap-3">
        <Progress value={progress.percent} className="h-1.5 flex-1" />
        <span className="text-muted-foreground w-12 text-right font-mono text-xs">
          {progress.label}
        </span>
        {done !== undefined && total !== undefined ? (
          <span className="text-muted-foreground hidden w-32 text-right font-mono text-xs sm:block">
            {`${formatBytes(done)} / ${formatBytes(total)}`}
          </span>
        ) : null}
      </div>
    </li>
  );
}
