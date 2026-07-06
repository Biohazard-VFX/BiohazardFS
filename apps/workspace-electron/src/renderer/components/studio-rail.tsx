import { Plus } from 'lucide-react';
import { useState } from 'react';

import { Logo } from '@/components/logo';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import { cn } from '@/lib/utils';

// Far-left studio rail. The spec's centerpiece (DASHBOARD_UX §4.1): a vertical
// rail of connected-studio profiles with at-a-glance health, plus "Add Studio".
//
// MVP: a single profile backed by the local daemon. auth.enroll / multi-mount
// are daemon-gated (method_not_implemented), so "Add Studio" opens an honest
// stub dialog rather than a flow that can't complete. The IA is in place for
// real multi-studio once the daemon supports it.
export function StudioRail({
  reachable,
  onShowOnboarding,
}: {
  reachable: boolean;
  onShowOnboarding: () => void;
}) {
  const [addOpen, setAddOpen] = useState(false);
  return (
    <aside className="bg-sidebar flex w-14 shrink-0 flex-col items-center gap-2 border-r py-3">
      {/* Active profile. Single profile = the local daemon studio. */}
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            className="relative rounded-lg"
            aria-label="Local workspace (connected)"
          >
            <span className="ring-primary flex size-9 items-center justify-center rounded-lg bg-card">
              <Logo className="h-5" />
            </span>
            {/* Health dot: green reachable, red not. */}
            <span
              className={cn(
                'absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full border-2 border-sidebar',
                reachable ? 'bg-emerald-500' : 'bg-destructive',
              )}
              aria-hidden="true"
            />
          </button>
        </TooltipTrigger>
        <TooltipContent side="right">Local workspace</TooltipContent>
      </Tooltip>

      <div className="flex-1" />

      {/* Add Studio — gated (auth.enroll / multi-mount not implemented). */}
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="icon"
            className="text-muted-foreground size-9"
            onClick={() => {
              setAddOpen(true);
            }}
            aria-label="Add a studio"
          >
            <Plus className="size-4" />
          </Button>
        </TooltipTrigger>
        <TooltipContent side="right">Add a studio</TooltipContent>
      </Tooltip>

      <Dialog open={addOpen} onOpenChange={setAddOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Add a studio</DialogTitle>
            <DialogDescription>
              Connecting to another studio requires server-side enrollment and per-studio mounts,
              which the daemon doesn&apos;t support yet (
              <code className="font-mono">auth.enroll</code>,{' '}
              <code className="font-mono">mount.attach</code> return method_not_implemented).
              Multi-studio will land here once that work ships.
            </DialogDescription>
          </DialogHeader>
          <div className="flex justify-between gap-2">
            <Button
              variant="ghost"
              onClick={() => {
                setAddOpen(false);
                onShowOnboarding();
              }}
            >
              Preview first-run
            </Button>
            <Button
              variant="outline"
              onClick={() => {
                setAddOpen(false);
              }}
            >
              Got it
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </aside>
  );
}
