import { Minus, Square, X } from 'lucide-react';

import { cn } from '@/lib/utils';

// Native-style window controls for the frameless chrome. Each button is
// `no-drag` so clicks register even though the parent topbar is a drag region.
export function WindowControls() {
  return (
    <div
      className="flex items-center"
      style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}
    >
      <ControlButton
        label="Minimize"
        onClick={() => {
          window.biohazardfs.minimizeWindow();
        }}
      >
        <Minus className="size-3.5" />
      </ControlButton>
      <ControlButton
        label="Maximize"
        onClick={() => {
          window.biohazardfs.toggleMaximize();
        }}
      >
        <Square className="size-3" />
      </ControlButton>
      <ControlButton
        label="Close"
        variant="close"
        onClick={() => {
          window.biohazardfs.closeWindow();
        }}
      >
        <X className="size-3.5" />
      </ControlButton>
    </div>
  );
}

function ControlButton({
  label,
  onClick,
  variant = 'default',
  children,
}: {
  label: string;
  onClick: () => void;
  variant?: 'default' | 'close';
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className={cn(
        'hover:bg-accent text-muted-foreground inline-flex h-7 w-9 items-center justify-center rounded-sm transition-colors',
        variant === 'close' && 'hover:bg-destructive hover:text-destructive-foreground',
      )}
    >
      {children}
    </button>
  );
}
