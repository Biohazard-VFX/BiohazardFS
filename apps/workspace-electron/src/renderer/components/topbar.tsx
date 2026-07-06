import { RefreshCw, Search } from 'lucide-react';
import type { CSSProperties } from 'react';

import { VIEW_TITLES, type FilesViewMode, type ViewId } from '@/app/nav';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { SyncStatus } from '@/components/sync-status';
import { WindowControls } from '@/components/window-controls';
import { FilesViewToggle } from '@/components/views/files-view';

type Props = {
  view: ViewId;
  query: string;
  onQueryChange: (query: string) => void;
  refreshing: boolean;
  onRefresh: () => void;
  transferCount: number;
  dirtyCount: number;
  reachable: boolean;
  frameless: boolean;
  filesMode: FilesViewMode;
  onFilesModeChange: (mode: FilesViewMode) => void;
};

// When frameless, the header is the window's drag region. Interactive children
// (search input, buttons) opt out with WebkitAppRegion: no-drag so they still
// receive clicks.
export function Topbar({
  view,
  query,
  onQueryChange,
  refreshing,
  onRefresh,
  transferCount,
  dirtyCount,
  reachable,
  frameless,
  filesMode,
  onFilesModeChange,
}: Props) {
  const showSearch = view === 'drive' || view === 'cache' || view === 'conflicts';
  const dragStyle = frameless ? ({ WebkitAppRegion: 'drag' } as CSSProperties) : undefined;
  const noDragStyle = { WebkitAppRegion: 'no-drag' } as CSSProperties;
  return (
    <header className="flex h-14 shrink-0 items-center gap-3 border-b pl-4 pr-2" style={dragStyle}>
      <h1 className="text-sm font-semibold tracking-tight select-none">{VIEW_TITLES[view]}</h1>
      <SyncStatus transferCount={transferCount} dirtyCount={dirtyCount} reachable={reachable} />
      <div className="flex-1" />
      {view === 'drive' ? (
        <div style={noDragStyle}>
          <FilesViewToggle mode={filesMode} onChange={onFilesModeChange} />
        </div>
      ) : null}
      {showSearch ? (
        <div className="relative w-64" style={noDragStyle}>
          <Search className="text-muted-foreground pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2" />
          <Input
            value={query}
            onChange={(e) => {
              onQueryChange(e.target.value);
            }}
            placeholder={`Filter ${view}…`}
            className="h-7 pl-8 text-xs"
            aria-label={`Filter ${view}`}
          />
        </div>
      ) : null}
      <div style={noDragStyle}>
        <Button
          variant="ghost"
          size="icon"
          onClick={onRefresh}
          disabled={refreshing}
          title="Refresh"
          aria-label="Refresh"
        >
          <RefreshCw className={refreshing ? 'size-4 animate-spin' : 'size-4'} />
        </Button>
      </div>
      {frameless ? <WindowControls /> : null}
    </header>
  );
}
