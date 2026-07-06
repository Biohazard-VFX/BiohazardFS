// View identifiers and navigation groups for the studio-scoped sidebar. The
// shell holds a single `ViewId` in state and swaps the view surface.
//
// Main nav = the artist-facing surfaces (DASHBOARD_UX §4.2). Admin nav =
// permission-gated pages; ones backed by real daemon reads are enabled, the
// rest render as reserved/disabled stubs so the IA leaves space (§5.7).

import type { LucideIcon } from 'lucide-react';
import {
  Activity,
  AlertTriangle,
  ArrowUpDown,
  Cable,
  Database,
  FolderTree,
  HardDrive,
  KeyRound,
  LayoutDashboard,
  Lock,
  MonitorSmartphone,
  Settings,
  ShieldCheck,
  Users,
} from 'lucide-react';

export type ViewId =
  | 'my-work'
  | 'connection'
  | 'drive'
  | 'transfers'
  | 'cache'
  | 'conflicts'
  | 'locks'
  | 'settings'
  | 'audit'
  | 'snapshots'
  | 'access'
  | 'members'
  | 'devices'
  | 'permissions'
  | 'storage';

// Files browser layout: drill-into-folders ("list") or expandable whole-tree.
export type FilesViewMode = 'list' | 'tree';

export type NavItem = {
  id: ViewId;
  label: string;
  icon: LucideIcon;
  // When true, the daemon doesn't back this view yet → render disabled with a
  // "requires daemon support" state rather than routing to an empty view.
  stubbed?: boolean;
  badgeKind?: 'dirty' | 'conflicts' | 'locks' | 'transfers';
};

export const MAIN_ITEMS: readonly NavItem[] = [
  { id: 'my-work', label: 'My Work', icon: LayoutDashboard },
  { id: 'connection', label: 'Connection', icon: Cable },
  { id: 'drive', label: 'Drive', icon: FolderTree, badgeKind: 'dirty' },
  { id: 'transfers', label: 'Transfers', icon: ArrowUpDown, badgeKind: 'transfers' },
  { id: 'cache', label: 'Cache', icon: HardDrive },
  { id: 'conflicts', label: 'Conflicts', icon: AlertTriangle, badgeKind: 'conflicts' },
  { id: 'locks', label: 'Locks', icon: Lock, badgeKind: 'locks' },
  { id: 'settings', label: 'Settings', icon: Settings },
];

export const ADMIN_ITEMS: readonly NavItem[] = [
  { id: 'audit', label: 'Audit', icon: Activity },
  { id: 'snapshots', label: 'Snapshots', icon: Database },
  { id: 'access', label: 'Access', icon: KeyRound },
  { id: 'members', label: 'Members', icon: Users, stubbed: true },
  { id: 'devices', label: 'Devices', icon: MonitorSmartphone, stubbed: true },
  { id: 'permissions', label: 'Permissions', icon: ShieldCheck, stubbed: true },
  { id: 'storage', label: 'Storage', icon: HardDrive, stubbed: true },
];

export const VIEW_TITLES: Record<ViewId, string> = {
  'my-work': 'My Work',
  connection: 'Connection',
  drive: 'Drive',
  transfers: 'Transfers',
  cache: 'Cache',
  conflicts: 'Conflicts',
  locks: 'Locks',
  settings: 'Settings',
  audit: 'Audit',
  snapshots: 'Snapshots',
  access: 'Access',
  members: 'Members',
  devices: 'Devices',
  permissions: 'Permissions',
  storage: 'Storage',
};
