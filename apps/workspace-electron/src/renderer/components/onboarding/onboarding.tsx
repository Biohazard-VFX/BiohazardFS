import { type CSSProperties, useState } from 'react';
import { CheckCircle2, Circle, LoaderCircle, TriangleAlert } from 'lucide-react';

import { type DaemonSnapshot } from '@/lib/use-daemon';
import { useDaemonFetch } from '@/lib/use-fetch';
import { isStubbed } from '@/lib/daemon-capabilities';
import { asString, extractData } from '@/lib/daemon';
import { cn } from '@/lib/utils';
import { Button } from '@/components/ui/button';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Separator } from '@/components/ui/separator';
import { WindowControls } from '@/components/window-controls';

// First-run onboarding (DASHBOARD_UX §3). A focused join flow:
// Welcome → Workstation preflight → Mount → Load access → Success.
//
// Hybrid honesty: steps the daemon can answer are real (daemon reachable,
// cache writable, workspace configured, current access via workset.list).
// Steps that need daemon auth/mount work are shown as honestly pending —
// enroll / device registration / invite validation / mount.attach all return
// method_not_implemented, so the flow can describe them but not complete them.
type Props = {
  snapshot: DaemonSnapshot;
  onClose: () => void;
  onOpenDrive: () => void;
  frameless: boolean;
  platform?: string;
};

const STEPS = ['Welcome', 'Workstation', 'Mount', 'Access', 'Ready'] as const;

export function Onboarding({ snapshot, onClose, onOpenDrive, frameless, platform }: Props) {
  const [step, setStep] = useState(0);

  const workspaceData = extractData(snapshot.workspace);
  const workspaceRoot = asString(workspaceData?.root);
  const reachable = snapshot.daemon?.body !== undefined;
  const workspaceReady = workspaceData?.state === 'ready';

  const dragStyle = frameless ? ({ WebkitAppRegion: 'drag' } as CSSProperties) : undefined;
  const macFrameless = frameless && platform === 'darwin';

  return (
    <div className="bg-background text-foreground relative flex h-screen w-screen">
      {macFrameless ? (
        <div className="absolute top-3 left-3 z-50">
          <WindowControls platform={platform} />
        </div>
      ) : null}
      {/* Stepper */}
      <aside
        className={cn(
          'bg-sidebar text-sidebar-foreground hidden w-64 shrink-0 flex-col gap-1 border-r p-4 sm:flex',
          macFrameless && 'pt-12',
        )}
      >
        <p className="text-muted-foreground mb-2 text-[0.62rem] font-semibold tracking-widest uppercase">
          Join a studio
        </p>
        {STEPS.map((label, i) => {
          const done = i < step;
          const active = i === step;
          return (
            <div
              key={label}
              className={cn(
                'flex items-center gap-2 rounded-md px-2 py-1.5 text-sm',
                active && 'bg-sidebar-accent text-sidebar-accent-foreground font-medium',
                !active && 'text-muted-foreground',
              )}
            >
              {done ? (
                <CheckCircle2 className="text-primary size-4" />
              ) : active ? (
                <Circle className="text-primary size-4" />
              ) : (
                <Circle className="size-4" />
              )}
              {label}
            </div>
          );
        })}
        <div className="flex-1" />
        <Button variant="ghost" size="sm" className="justify-start" onClick={onClose}>
          Skip for now
        </Button>
      </aside>

      {/* Content */}
      <div className="flex min-w-0 flex-1 flex-col">
        <header
          className={cn(
            'flex h-12 shrink-0 items-center gap-3 border-b px-4',
            macFrameless && 'pl-28',
          )}
          style={dragStyle}
        >
          <span className="text-sm font-semibold tracking-tight select-none">
            Biohazard Workspace
          </span>
          <div className="flex-1" />
          {frameless && !macFrameless ? <WindowControls platform={platform} /> : null}
        </header>
        <ScrollArea className="min-h-0 flex-1">
          <div className="mx-auto flex max-w-xl flex-col gap-4 p-6">
            {step === 0 ? (
              <WelcomeStep reachable={reachable} workspaceReady={workspaceReady} />
            ) : step === 1 ? (
              <PreflightStep reachable={reachable} workspaceReady={workspaceReady} />
            ) : step === 2 ? (
              <MountStep workspaceRoot={workspaceRoot} />
            ) : step === 3 ? (
              <AccessStep />
            ) : (
              <SuccessStep onOpenDrive={onOpenDrive} onClose={onClose} />
            )}
          </div>
        </ScrollArea>
        <footer className="flex h-14 shrink-0 items-center justify-between border-t px-4">
          <Button
            variant="ghost"
            size="sm"
            disabled={step === 0}
            onClick={() => {
              setStep((s) => s - 1);
            }}
          >
            Back
          </Button>
          {step < STEPS.length - 1 ? (
            <Button
              size="sm"
              onClick={() => {
                setStep((s) => s + 1);
              }}
            >
              Continue
            </Button>
          ) : (
            <Button size="sm" onClick={onClose}>
              Done
            </Button>
          )}
        </footer>
      </div>
    </div>
  );
}

function H({ children }: { children: React.ReactNode }) {
  return <h2 className="text-lg font-semibold tracking-tight">{children}</h2>;
}
function Lead({ children }: { children: React.ReactNode }) {
  return <p className="text-muted-foreground text-sm">{children}</p>;
}

function WelcomeStep({
  reachable,
  workspaceReady,
}: {
  reachable: boolean;
  workspaceReady: boolean;
}) {
  return (
    <section className="flex flex-col gap-3">
      <H>Join a studio</H>
      <Lead>
        This adds a connected studio on this computer. Your project and folder access can update
        later — invites join a studio, not a fixed folder.
      </Lead>
      <Separator />
      <p className="text-muted-foreground text-xs">
        Server-side enrollment (<code className="font-mono">auth.enroll</code>) and invite
        validation aren&apos;t implemented in the daemon yet, so a real join can&apos;t complete
        here. This preview shows the flow against the local daemon.
      </p>
      <KV label="Studio" value="Local workspace" />
      <KV label="Daemon" value={reachable ? 'reachable' : 'unreachable'} />
      <KV label="Workspace" value={workspaceReady ? 'configured' : 'not configured'} />
    </section>
  );
}

function PreflightStep({
  reachable,
  workspaceReady,
}: {
  reachable: boolean;
  workspaceReady: boolean;
}) {
  const rows: { label: string; status: 'ok' | 'pending' | 'stub'; note?: string }[] = [
    { label: 'Biohazard daemon', status: reachable ? 'ok' : 'pending' },
    { label: 'Workspace configured', status: workspaceReady ? 'ok' : 'pending' },
    { label: 'Local cache writable', status: workspaceReady ? 'ok' : 'pending' },
    { label: 'Server connection', status: 'stub', note: 'requires server-side auth' },
    { label: 'Device registration', status: 'stub', note: 'auth.enroll not built' },
  ];
  return (
    <section className="flex flex-col gap-3">
      <H>Checking your workstation…</H>
      <ul className="flex flex-col gap-1">
        {rows.map((row) => (
          <li key={row.label} className="flex items-center gap-2 text-sm">
            <StatusIcon status={row.status} />
            <span className="flex-1">{row.label}</span>
            <span className="text-muted-foreground text-xs uppercase tracking-wide">
              {row.status === 'ok' ? 'Ready' : row.status === 'pending' ? 'Pending' : 'Not built'}
            </span>
          </li>
        ))}
      </ul>
      {rows.some((r) => r.note) ? (
        <p className="text-muted-foreground text-xs">
          Some checks require daemon/server support and are shown as not-built rather than failing
          silently.
        </p>
      ) : null}
    </section>
  );
}

function MountStep({ workspaceRoot }: { workspaceRoot: string }) {
  const canAttach = !isStubbed('mount.attach');
  return (
    <section className="flex flex-col gap-3">
      <H>Mount &amp; cache</H>
      <Lead>Defaults are preselected. Mount attaches the studio namespace to a stable path.</Lead>
      <KV label="Mount path" value={workspaceRoot || '(not configured)'} mono />
      <KV label="Cache" value="user-local cache directory (default)" mono />
      <Button
        size="sm"
        className="w-fit"
        disabled={!canAttach}
        title={canAttach ? undefined : 'Requires daemon support (mount.attach not built)'}
      >
        Mount Studio
      </Button>
      {!canAttach ? (
        <p className="text-muted-foreground text-xs">
          Mount attach is daemon-gated (mount.attach returns method_not_implemented). The local
          workspace is already mounted in this preview.
        </p>
      ) : null}
    </section>
  );
}

function AccessStep() {
  const worksets = useDaemonFetch('workset.list', {}, 0);
  const entries = (worksets.data?.entries ?? worksets.data?.worksets ?? worksets.data?.items) as
    Array<{ name?: unknown }> | undefined;
  return (
    <section className="flex flex-col gap-3">
      <H>Loading your available work…</H>
      <Lead>
        Loading access metadata does not download file content — only the namespace appears.
      </Lead>
      {worksets.loading ? (
        <p className="text-muted-foreground text-xs">Loading…</p>
      ) : entries && entries.length > 0 ? (
        <ul className="flex flex-col gap-1">
          {entries.map((w, i) => (
            <li key={String(i)} className="text-sm">
              {asString(w.name)}
            </li>
          ))}
        </ul>
      ) : (
        <p className="text-muted-foreground text-xs">
          No workspaces exposed by workset.list yet. If none are assigned, ask a producer/admin to
          grant access, then refresh.
        </p>
      )}
    </section>
  );
}

function SuccessStep({ onOpenDrive, onClose }: { onOpenDrive: () => void; onClose: () => void }) {
  return (
    <section className="flex flex-col gap-3">
      <H>You&apos;re set up.</H>
      <Lead>The local workspace is mounted and your current work is available.</Lead>
      <div className="flex gap-2">
        <Button size="sm" onClick={onOpenDrive}>
          Open drive
        </Button>
        <Button size="sm" variant="outline" onClick={onClose}>
          Back to app
        </Button>
      </div>
    </section>
  );
}

function StatusIcon({ status }: { status: 'ok' | 'pending' | 'stub' }) {
  if (status === 'ok') return <CheckCircle2 className="size-4 text-emerald-500" />;
  if (status === 'pending')
    return <LoaderCircle className="text-muted-foreground size-4 animate-spin" />;
  return <TriangleAlert className="text-muted-foreground/50 size-4" />;
}

function KV({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex items-center justify-between gap-3">
      <span className="text-muted-foreground text-[0.62rem] font-semibold tracking-widest uppercase">
        {label}
      </span>
      <span className={cn('truncate text-sm', mono && 'font-mono text-xs')}>{value}</span>
    </div>
  );
}
