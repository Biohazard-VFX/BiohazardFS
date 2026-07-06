import { AlertCircle, CheckCircle2 } from 'lucide-react';

import { cn } from '@/lib/utils';
import { Skeleton } from '@/components/ui/skeleton';

// Shared empty/loading/error/feedback surfaces. Every async view must render
// one of these rather than a blank panel (CLAUDE.md: empty/loading/error states
// are required for every async UI surface).

export function ViewLoading({ rows = 4 }: { rows?: number }) {
  return (
    <div className="flex flex-col gap-2 p-4">
      {Array.from({ length: rows }).map((_, i) => (
        <Skeleton key={i} className="h-9 w-full" />
      ))}
    </div>
  );
}

export function ViewEmpty({
  title,
  children,
  icon,
}: {
  title: string;
  children?: React.ReactNode;
  icon?: React.ReactNode;
}) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-2 p-10 text-center">
      {icon ? <div className="text-muted-foreground/40 mb-1 [&_svg]:size-7">{icon}</div> : null}
      <p className="text-sm font-medium">{title}</p>
      {children ? <p className="text-muted-foreground max-w-sm text-xs">{children}</p> : null}
    </div>
  );
}

export function ViewError({
  label,
  error,
}: {
  label: string;
  error: { code: string; message: string };
}) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-2 p-10 text-center">
      <AlertCircle className="text-destructive size-5" />
      <p className="text-sm font-medium">{label}</p>
      <p className="text-muted-foreground max-w-sm text-xs">{error.message}</p>
      <code className="bg-muted text-muted-foreground rounded px-1.5 py-0.5 text-[0.65rem]">
        {error.code}
      </code>
    </div>
  );
}

export function Feedback({ ok, children }: { ok: boolean; children: React.ReactNode }) {
  const Icon = ok ? CheckCircle2 : AlertCircle;
  return (
    <p
      className={cn(
        'flex items-center gap-1.5 px-4 py-1.5 text-xs',
        ok ? 'text-muted-foreground' : 'text-primary',
      )}
    >
      <Icon className={ok ? 'size-3.5' : 'size-3.5 text-primary'} />
      {children}
    </p>
  );
}
