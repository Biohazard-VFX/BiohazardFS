import { Minus, Square, X } from 'lucide-react';

import { cn } from '@/lib/utils';

type Props = {
  platform?: string;
};

// App-owned window controls for frameless chrome. Each button is `no-drag` so
// clicks register even though the parent topbar/header is a drag region.
export function WindowControls({ platform }: Props) {
  if (platform === 'darwin') {
    return (
      <div
        className="group/window-controls flex items-center gap-2 px-1"
        style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}
        aria-label="Window controls"
      >
        <MacControlButton
          label="Close"
          className="bg-[#ff5f57] ring-[#e0443e]"
          onClick={() => {
            window.biohazardfs.closeWindow();
          }}
        >
          <X className="size-2.5 opacity-0 group-hover/window-controls:opacity-70" />
        </MacControlButton>
        <MacControlButton
          label="Minimize"
          className="bg-[#ffbd2e] ring-[#dea123]"
          onClick={() => {
            window.biohazardfs.minimizeWindow();
          }}
        >
          <Minus className="size-2.5 opacity-0 group-hover/window-controls:opacity-70" />
        </MacControlButton>
        <MacControlButton
          label="Maximize"
          className="bg-[#28c840] ring-[#1aab29]"
          onClick={() => {
            window.biohazardfs.toggleMaximize();
          }}
        >
          <Square className="size-2 opacity-0 group-hover/window-controls:opacity-70" />
        </MacControlButton>
      </div>
    );
  }

  return (
    <div
      className="flex items-center"
      style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}
      aria-label="Window controls"
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

function MacControlButton({
  label,
  onClick,
  className,
  children,
}: {
  label: string;
  onClick: () => void;
  className: string;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className={cn(
        'inline-flex size-3 items-center justify-center rounded-full text-black/80 ring-1 transition-transform hover:scale-110 active:scale-95',
        className,
      )}
    >
      {children}
    </button>
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
