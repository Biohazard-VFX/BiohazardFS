import { useCallback, useEffect, useRef, useState } from 'react';
import {
  AlertTriangle,
  ChevronDown,
  ChevronRight,
  CloudOff,
  Folder,
  List,
  ListTree,
  LoaderCircle,
  Pin,
} from 'lucide-react';

import { type FilesViewMode } from '@/app/nav';
import { useActions } from '@/app/root';
import { type DaemonSnapshot } from '@/lib/use-daemon';
import {
  type DaemonStatusResult,
  type Entry,
  asString,
  entryList,
  extractData,
  extractError,
  isDirtyEntry,
  keepLastGood,
} from '@/lib/daemon';
import { formatBytes } from '@/lib/format';
import { cn } from '@/lib/utils';
import { Button } from '@/components/ui/button';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import { ViewEmpty, ViewError, ViewLoading } from '@/components/view-states';
import { FileTypeIcon } from '@/components/file-type-icon';

type Props = {
  query: string;
  snapshot: DaemonSnapshot;
  loaded: boolean;
  refreshNonce: number;
  mode: FilesViewMode;
};

// Two layouts over the same workspace data:
//  - list: drill into one folder at a time (breadcrumbs + back/forward).
//  - tree: expandable whole-tree overview with lazy-loaded directory children.
export function FilesView({ query, snapshot, loaded, refreshNonce, mode }: Props) {
  return mode === 'list' ? (
    <ListView query={query} snapshot={snapshot} loaded={loaded} refreshNonce={refreshNonce} />
  ) : (
    <TreeView
      rootList={snapshot.workspaceList}
      loaded={loaded}
      refreshNonce={refreshNonce}
      query={query}
    />
  );
}

// ------------------------------ shared bits ------------------------------

function isDirectory(entry: Entry): boolean {
  const kind = asString(entry.kind).toLowerCase();
  return kind === 'dir' || kind === 'directory' || kind === 'folder';
}

// Directories first, then files, alphabetical within each group.
function sortEntries(entries: Entry[]): Entry[] {
  return [...entries].sort((a, b) => {
    const aDir = isDirectory(a);
    const bDir = isDirectory(b);
    if (aDir !== bDir) return aDir ? -1 : 1;
    return asString(a.name).localeCompare(asString(b.name));
  });
}

function joinPath(parent: string, name: string): string {
  return parent === '' ? name : `${parent}/${name}`;
}

// Row icon: folder for dirs, red alert for dirty (safety override), spinner for
// active transfer, otherwise the per-extension file-type icon.
function RowIcon({ entry, name }: { entry: Entry; name: string }) {
  const base = 'size-4 shrink-0';
  if (isDirectory(entry)) return <Folder className={cn(base, 'text-muted-foreground')} />;
  if (isDirtyEntry(entry)) return <AlertTriangle className={cn(base, 'text-primary')} />;
  const state = asString(entry.state);
  if (state === 'running' || state === 'uploading' || state === 'downloading') {
    return <LoaderCircle className={cn(base, 'text-muted-foreground animate-spin')} />;
  }
  return <FileTypeIcon name={name} className={base} />;
}

// Hover-revealed pin / dehydrate actions + transient feedback. Shared by both
// list and tree rows so the safety logic (dirty dehydrate guard) lives once.
function RowActions({ entry, path }: { entry: Entry; path: string }) {
  const { pinEntry, dehydrateEntry } = useActions();
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<{ ok: boolean; text: string } | null>(null);

  async function act(kind: 'pin' | 'dehydrate') {
    setFeedback(null);
    if (kind === 'dehydrate' && isDirtyEntry(entry)) {
      // Safety invariant: never remove a local copy with unsynced changes.
      setFeedback({ ok: false, text: 'Local changes haven’t synced. Keeping the local copy.' });
      return;
    }
    setBusy(true);
    const result = kind === 'pin' ? await pinEntry(path) : await dehydrateEntry(path);
    setBusy(false);
    const err = extractError(result);
    setFeedback(
      err
        ? {
            ok: false,
            text: `${kind === 'pin' ? 'Make available offline' : 'Remove local copy'} failed (${err.code}).`,
          }
        : {
            ok: true,
            text: kind === 'pin' ? 'Available offline.' : 'Local copy removed.',
          },
    );
  }

  if (isDirectory(entry)) return null;
  return (
    <div className="flex items-center gap-1.5">
      {feedback ? (
        <span className={cn('text-xs', feedback.ok ? 'text-muted-foreground' : 'text-primary')}>
          {feedback.text}
        </span>
      ) : null}
      <div
        className={cn(
          'flex items-center gap-0.5 transition-opacity',
          feedback ? 'hidden' : 'opacity-0 focus-within:opacity-100 group-hover:opacity-100',
        )}
      >
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="icon"
              className="size-7"
              disabled={busy}
              onClick={() => void act('pin')}
              aria-label="Make available offline"
            >
              <Pin className="size-3.5" />
            </Button>
          </TooltipTrigger>
          <TooltipContent>Make available offline</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="icon"
              className="size-7"
              disabled={isDirtyEntry(entry) || busy}
              onClick={() => void act('dehydrate')}
              aria-label="Remove local copy"
            >
              <CloudOff className="size-3.5" />
            </Button>
          </TooltipTrigger>
          <TooltipContent>
            {isDirtyEntry(entry)
              ? 'Local changes haven’t synced yet'
              : 'Remove the local copy (cloud data stays)'}
          </TooltipContent>
        </Tooltip>
      </div>
    </div>
  );
}

// ------------------------------- list view -------------------------------

function ListView({
  query,
  snapshot,
  loaded,
  refreshNonce,
}: {
  query: string;
  snapshot: DaemonSnapshot;
  loaded: boolean;
  refreshNonce: number;
}) {
  const [cwd, setCwd] = useState('');
  const [list, setList] = useState<DaemonStatusResult | null>(snapshot.workspaceList);
  const navigatingRef = useRef(false);

  // When the shell refetches the root list, adopt it if we're at the root and
  // not mid-navigation (otherwise we'd overwrite the destination listing).
  useEffect(() => {
    if (cwd === '' && !navigatingRef.current) {
      setList((prev) => keepLastGood(prev, snapshot.workspaceList));
    }
  }, [snapshot.workspaceList, cwd]);

  // Manual refresh re-fetches the CURRENT directory, not just root.
  const cwdRef = useRef(cwd);
  cwdRef.current = cwd;
  useEffect(() => {
    if (refreshNonce === 0) return;
    let cancelled = false;
    void window.biohazardfs.workspaceList(cwdRef.current).then((result) => {
      if (!cancelled) setList((prev) => keepLastGood(prev, result));
    });
    return () => {
      cancelled = true;
    };
  }, [refreshNonce]);

  const navigate = (path: string) => {
    void (async () => {
      navigatingRef.current = true;
      try {
        const result = await window.biohazardfs.workspaceList(path);
        setList((prev) => keepLastGood(prev, result));
        setCwd(path);
      } finally {
        navigatingRef.current = false;
      }
    })();
  };

  const data = extractData(list);
  const error = extractError(list);
  const entries = entryList(data, ['entries', 'items']);
  const filtered = query
    ? entries.filter((e) => asString(e.name).toLowerCase().includes(query.toLowerCase()))
    : entries;
  const sorted = sortEntries(filtered);
  const initialLoading = !loaded && list === null;

  return (
    <div className="flex h-full flex-col">
      <Breadcrumbs cwd={cwd} onNavigate={navigate} />
      <ScrollArea className="min-h-0 flex-1">
        {initialLoading ? (
          <ViewLoading rows={6} />
        ) : error && entries.length === 0 ? (
          <ViewError label="Couldn't list files" error={error} />
        ) : sorted.length === 0 ? (
          <ViewEmpty title={query ? 'No matches' : 'This folder is empty'} icon={<Folder />}>
            {query
              ? `Nothing here matches “${query}”.`
              : 'Mount a workspace to browse shots and assets.'}
          </ViewEmpty>
        ) : (
          <ul className="divide-border divide-y">
            {sorted.map((entry, index) => {
              const name = asString(entry.name, '(unnamed)');
              return (
                <FileRow
                  key={`${String(index)}:${name}`}
                  entry={entry}
                  path={joinPath(cwd, name)}
                  dir={isDirectory(entry)}
                  name={name}
                  onNavigate={navigate}
                />
              );
            })}
          </ul>
        )}
      </ScrollArea>
    </div>
  );
}

function Breadcrumbs({ cwd, onNavigate }: { cwd: string; onNavigate: (path: string) => void }) {
  const segments = cwd.split('/').filter(Boolean);
  return (
    <div className="flex h-9 shrink-0 items-center gap-0.5 border-b px-3 text-xs">
      <button
        type="button"
        className="text-muted-foreground hover:text-foreground rounded px-1.5 py-0.5"
        onClick={() => {
          onNavigate('');
        }}
      >
        Workspace
      </button>
      {segments.map((seg, i) => {
        const path = segments.slice(0, i + 1).join('/');
        const last = i === segments.length - 1;
        return (
          <span key={path} className="flex items-center gap-0.5">
            <ChevronRight className="text-muted-foreground size-3" />
            <button
              type="button"
              className={cn(
                'rounded px-1.5 py-0.5',
                last
                  ? 'text-foreground font-medium'
                  : 'text-muted-foreground hover:text-foreground',
              )}
              onClick={() => {
                onNavigate(path);
              }}
            >
              {seg}
            </button>
          </span>
        );
      })}
    </div>
  );
}

function FileRow({
  entry,
  path,
  dir,
  name,
  onNavigate,
}: {
  entry: Entry;
  path: string;
  dir: boolean;
  name: string;
  onNavigate: (path: string) => void;
}) {
  return (
    <li className="group hover:bg-accent/40 flex items-center gap-3 px-3 py-1.5">
      <button
        type="button"
        className="flex min-w-0 flex-1 items-center gap-2.5 text-left"
        onClick={() => {
          if (dir) onNavigate(path);
        }}
        disabled={!dir}
      >
        <RowIcon entry={entry} name={name} />
        <span className="truncate text-sm font-medium">{name}</span>
        {dir ? (
          <span className="text-muted-foreground text-xs">folder</span>
        ) : entry.size_bytes !== undefined ? (
          <span className="text-muted-foreground text-xs">{formatBytes(entry.size_bytes)}</span>
        ) : null}
      </button>
      <RowActions entry={entry} path={path} />
    </li>
  );
}

// ------------------------------- tree view -------------------------------

function TreeView({
  rootList,
  loaded,
  refreshNonce,
  query,
}: {
  rootList: DaemonStatusResult | null;
  loaded: boolean;
  refreshNonce: number;
  query: string;
}) {
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  // Nullable entries: an unset path is genuinely undefined at runtime, so the
  // type must allow that (Record index otherwise reads as always-defined).
  const [children, setChildren] = useState<Record<string, Entry[] | undefined>>({});
  const [loading, setLoading] = useState<Set<string>>(new Set());

  const expandedRef = useRef(expanded);
  expandedRef.current = expanded;

  const fetchDir = useCallback(async (path: string) => {
    setLoading((prev) => new Set(prev).add(path));
    try {
      const result = await window.biohazardfs.workspaceList(path);
      const entries = entryList(extractData(result), ['entries', 'items']);
      setChildren((prev) => ({ ...prev, [path]: entries }));
    } finally {
      setLoading((prev) => {
        const n = new Set(prev);
        n.delete(path);
        return n;
      });
    }
  }, []);

  const toggle = useCallback(
    (path: string) => {
      setExpanded((prev) => {
        const n = new Set(prev);
        if (n.has(path)) {
          n.delete(path);
        } else {
          n.add(path);
        }
        return n;
      });
      if (!children[path] && !loading.has(path)) {
        void fetchDir(path);
      }
    },
    [children, loading, fetchDir],
  );

  // Manual refresh: re-fetch every expanded directory so the tree updates.
  useEffect(() => {
    if (refreshNonce === 0) return;
    for (const p of expandedRef.current) {
      void fetchDir(p);
    }
  }, [refreshNonce, fetchDir]);

  // ---- Tree-aware search ----
  // When a filter is active, recursively crawl every directory (depth-capped)
  // and render only branches that contain a match, auto-expanded. A generation
  // counter cancels stale crawls when the query changes mid-flight.
  const [searchChildren, setSearchChildren] = useState<Record<string, Entry[] | undefined>>({});
  const [searching, setSearching] = useState(false);
  const searchGen = useRef(0);
  const q = query.trim().toLowerCase();

  useEffect(() => {
    if (q === '') {
      searchGen.current += 1;
      setSearchChildren({});
      setSearching(false);
      return;
    }
    const gen = ++searchGen.current;
    setSearching(true);
    const SEARCH_DEPTH_LIMIT = 12;
    const crawl = async (path: string, depth: number) => {
      // Stale crawl (query changed mid-flight)? Bail. The generation counter
      // alone is enough to cancel outdated work.
      if (gen !== searchGen.current) return;
      const result = await window.biohazardfs.workspaceList(path);
      if (gen !== searchGen.current) return;
      const entries = entryList(extractData(result), ['entries', 'items']);
      setSearchChildren((prev) => ({ ...prev, [path]: entries }));
      if (depth >= SEARCH_DEPTH_LIMIT) return;
      await Promise.all(
        entries
          .filter((e) => isDirectory(e))
          .map((d) => crawl(joinPath(path, asString(d.name)), depth + 1)),
      );
    };
    void crawl('', 0).finally(() => {
      if (gen === searchGen.current) setSearching(false);
    });
    // No cleanup needed: the generation counter invalidates stale crawls.
    // query is the only dependency; refreshNonce re-crawls via the key below.
  }, [q, refreshNonce]);

  const data = extractData(rootList);
  const error = extractError(rootList);
  const rootEntries = entryList(data, ['entries', 'items']);
  const initialLoading = !loaded && rootList === null;

  if (initialLoading) return <ViewLoading rows={8} />;
  if (error && rootEntries.length === 0)
    return <ViewError label="Couldn't list files" error={error} />;
  if (rootEntries.length === 0) {
    return (
      <ViewEmpty title="Workspace is empty" icon={<Folder />}>
        Mount a workspace to browse shots and assets.
      </ViewEmpty>
    );
  }

  // Search mode: filtered, auto-expanded render over the crawled tree.
  if (q !== '') {
    const visibleRoots = sortEntries(rootEntries).filter((e) =>
      subtreeMatches(e, '', q, searchChildren),
    );
    return (
      <ScrollArea className="h-full">
        {searching && visibleRoots.length === 0 ? (
          <p className="text-muted-foreground p-4 text-xs">Searching…</p>
        ) : visibleRoots.length === 0 ? (
          <ViewEmpty title="No matches" icon={<Folder />}>
            {`Nothing in the workspace matches “${query}”.`}
          </ViewEmpty>
        ) : (
          <ul className="py-1">
            {visibleRoots.map((entry, index) => (
              <SearchTreeRow
                key={`${String(index)}:${asString(entry.name)}`}
                entry={entry}
                parentPath=""
                depth={0}
                query={q}
                childrenMap={searchChildren}
              />
            ))}
          </ul>
        )}
      </ScrollArea>
    );
  }

  return (
    <ScrollArea className="h-full">
      <ul className="py-1">
        {sortEntries(rootEntries).map((entry, index) => (
          <TreeRow
            key={`${String(index)}:${asString(entry.name)}`}
            entry={entry}
            parentPath=""
            depth={0}
            expanded={expanded}
            loading={loading}
            childrenMap={children}
            onToggle={toggle}
          />
        ))}
      </ul>
    </ScrollArea>
  );
}

// A node matches the search if its own name contains the query, or any
// descendant (in the crawled searchChildren map) does.
function subtreeMatches(
  entry: Entry,
  parentPath: string,
  q: string,
  childrenMap: Record<string, Entry[] | undefined>,
): boolean {
  const name = asString(entry.name).toLowerCase();
  if (name.includes(q)) return true;
  if (isDirectory(entry)) {
    const path = joinPath(parentPath, asString(entry.name));
    return (childrenMap[path] ?? []).some((child) => subtreeMatches(child, path, q, childrenMap));
  }
  return false;
}

function SearchTreeRow({
  entry,
  parentPath,
  depth,
  query,
  childrenMap,
}: {
  entry: Entry;
  parentPath: string;
  depth: number;
  query: string;
  childrenMap: Record<string, Entry[] | undefined>;
}) {
  const name = asString(entry.name, '(unnamed)');
  const path = joinPath(parentPath, name);
  const dir = isDirectory(entry);
  const matches = name.toLowerCase().includes(query);

  // Directory: show expanded with only the children that themselves match.
  if (dir) {
    const kids = childrenMap[path] ?? [];
    const visibleKids = sortEntries(kids).filter((k) =>
      subtreeMatches(k, path, query, childrenMap),
    );
    return (
      <>
        <li
          className="group hover:bg-accent/40 flex items-center gap-1.5 py-1 pr-2"
          style={{ paddingLeft: `${String(depth * 14 + 8)}px` }}
        >
          <span className="text-muted-foreground flex size-4 shrink-0 items-center justify-center">
            <ChevronDown className="size-3.5" />
          </span>
          <RowIcon entry={entry} name={name} />
          {matches ? (
            <HighlightedName name={name} query={query} />
          ) : (
            <span className="truncate text-sm font-medium">{name}</span>
          )}
        </li>
        {visibleKids.length > 0 ? (
          <li className="list-none p-0">
            <ul>
              {visibleKids.map((child, index) => (
                <SearchTreeRow
                  key={`${String(index)}:${asString(child.name)}`}
                  entry={child}
                  parentPath={path}
                  depth={depth + 1}
                  query={query}
                  childrenMap={childrenMap}
                />
              ))}
            </ul>
          </li>
        ) : null}
      </>
    );
  }

  return (
    <li
      className="group hover:bg-accent/40 flex items-center gap-1.5 py-1 pr-2"
      style={{ paddingLeft: `${String(depth * 14 + 8)}px` }}
    >
      <span className="inline-block size-4 shrink-0" aria-hidden="true" />
      <RowIcon entry={entry} name={name} />
      <div className="min-w-0 flex-1">
        <HighlightedName name={name} query={query} />
      </div>
      <span className="text-muted-foreground text-xs">
        {parentPath === '' ? '/ ' : `${parentPath}/ `}
      </span>
      {entry.size_bytes !== undefined ? (
        <span className="text-muted-foreground text-xs">{formatBytes(entry.size_bytes)}</span>
      ) : null}
      <RowActions entry={entry} path={path} />
    </li>
  );
}

function HighlightedName({ name, query }: { name: string; query: string }) {
  const idx = name.toLowerCase().indexOf(query);
  if (idx < 0) return <span className="truncate text-sm font-medium">{name}</span>;
  const before = name.slice(0, idx);
  const hit = name.slice(idx, idx + query.length);
  const after = name.slice(idx + query.length);
  return (
    <span className="truncate text-sm font-medium">
      {before}
      <mark className="bg-primary/25 text-foreground rounded px-0.5">{hit}</mark>
      {after}
    </span>
  );
}

function TreeRow({
  entry,
  parentPath,
  depth,
  expanded,
  loading,
  childrenMap,
  onToggle,
}: {
  entry: Entry;
  parentPath: string;
  depth: number;
  expanded: Set<string>;
  loading: Set<string>;
  childrenMap: Record<string, Entry[] | undefined>;
  onToggle: (path: string) => void;
}) {
  const name = asString(entry.name, '(unnamed)');
  const path = joinPath(parentPath, name);
  const dir = isDirectory(entry);
  const isOpen = expanded.has(path);
  const isLoading = loading.has(path);
  const kids = childrenMap[path];

  return (
    <>
      <li
        className="group hover:bg-accent/40 flex items-center gap-1.5 py-1 pr-2"
        style={{ paddingLeft: `${String(depth * 14 + 8)}px` }}
      >
        {dir ? (
          <button
            type="button"
            className="text-muted-foreground hover:text-foreground flex size-4 shrink-0 items-center justify-center"
            onClick={() => {
              onToggle(path);
            }}
            aria-label={isOpen ? 'Collapse' : 'Expand'}
          >
            {isLoading ? (
              <LoaderCircle className="size-3.5 animate-spin" />
            ) : isOpen ? (
              <ChevronDown className="size-3.5" />
            ) : (
              <ChevronRight className="size-3.5" />
            )}
          </button>
        ) : (
          <span className="inline-block size-4 shrink-0" aria-hidden="true" />
        )}
        <button
          type="button"
          className="flex min-w-0 flex-1 items-center gap-2.5 text-left"
          onClick={() => {
            if (dir) onToggle(path);
          }}
        >
          <RowIcon entry={entry} name={name} />
          <span className="truncate text-sm font-medium">{name}</span>
          {!dir && entry.size_bytes !== undefined ? (
            <span className="text-muted-foreground text-xs">{formatBytes(entry.size_bytes)}</span>
          ) : null}
        </button>
        <RowActions entry={entry} path={path} />
      </li>
      {dir && isOpen && kids ? (
        <li className="list-none p-0">
          <ul>
            {sortEntries(kids).map((child, index) => (
              <TreeRow
                key={`${String(index)}:${asString(child.name)}`}
                entry={child}
                parentPath={path}
                depth={depth + 1}
                expanded={expanded}
                loading={loading}
                childrenMap={childrenMap}
                onToggle={onToggle}
              />
            ))}
          </ul>
        </li>
      ) : null}
    </>
  );
}

// ------------------------------- view toggle ------------------------------
// (Rendered in the topbar; exported here so the icon choices live with the view.)

export function FilesViewToggle({
  mode,
  onChange,
}: {
  mode: FilesViewMode;
  onChange: (mode: FilesViewMode) => void;
}) {
  return (
    <div className="bg-muted/60 inline-flex items-center gap-0.5 rounded-md p-0.5">
      <ToggleBtn
        active={mode === 'list'}
        onClick={() => {
          onChange('list');
        }}
        label="List"
      >
        <List className="size-3.5" />
      </ToggleBtn>
      <ToggleBtn
        active={mode === 'tree'}
        onClick={() => {
          onChange('tree');
        }}
        label="Tree"
      >
        <ListTree className="size-3.5" />
      </ToggleBtn>
    </div>
  );
}

function ToggleBtn({
  active,
  onClick,
  label,
  children,
}: {
  active: boolean;
  onClick: () => void;
  label: string;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={label}
      title={label}
      aria-pressed={active}
      className={cn(
        'flex h-6 w-7 items-center justify-center rounded-sm transition-colors',
        active
          ? 'bg-background text-foreground shadow-xs'
          : 'text-muted-foreground hover:text-foreground',
      )}
    >
      {children}
    </button>
  );
}
