import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { Root } from '../root';

const unconfiguredSnapshot = {
  daemon: { body: { ok: true, data: {} } },
  workspace: { body: { ok: true, data: { state: 'unconfigured' } } },
  cacheStatus: null,
  cacheList: null,
  transferList: null,
  conflictList: null,
  lockList: null,
  versions: null,
};

vi.mock('@/lib/use-daemon', () => ({
  useDaemonState: () => ({
    snapshot: unconfiguredSnapshot,
    loaded: true,
    lastUpdated: null,
    refresh: vi.fn(() => Promise.resolve()),
  }),
}));

vi.mock('@/lib/use-prefs', () => ({
  useAppInfo: () => ({ frameless: false }),
  useTheme: () => undefined,
}));

vi.mock('@/lib/use-fetch', () => ({
  useDaemonFetch: () => ({ data: { entries: [] }, loading: false }),
}));

vi.mock('@/components/studio-rail', () => ({
  StudioRail: ({ onShowOnboarding }: { onShowOnboarding: () => void }) => (
    <button type="button" onClick={onShowOnboarding}>
      Onboarding
    </button>
  ),
}));
vi.mock('@/components/app-sidebar', () => ({ AppSidebar: () => <aside>Sidebar</aside> }));
vi.mock('@/components/topbar', () => ({ Topbar: () => <header>Topbar</header> }));
vi.mock('@/components/status-bar', () => ({ StatusBar: () => <footer>Status</footer> }));
vi.mock('@/components/views/my-work-view', () => ({ MyWorkView: () => <main>My work</main> }));
vi.mock('@/components/views/files-view', () => ({ FilesView: () => <main>Files</main> }));
vi.mock('@/components/views/activity-view', () => ({ ActivityView: () => null }));
vi.mock('@/components/views/cache-view', () => ({ CacheView: () => null }));
vi.mock('@/components/views/conflicts-view', () => ({ ConflictsView: () => null }));
vi.mock('@/components/views/connection-view', () => ({ ConnectionView: () => null }));
vi.mock('@/components/views/locks-view', () => ({ LocksView: () => null }));
vi.mock('@/components/views/audit-view', () => ({ AuditView: () => null }));
vi.mock('@/components/views/snapshots-view', () => ({ SnapshotsView: () => null }));
vi.mock('@/components/views/access-view', () => ({ AccessView: () => null }));
vi.mock('@/components/views/placeholder-view', () => ({ PlaceholderView: () => null }));
vi.mock('@/components/views/settings-view', () => ({ SettingsView: () => null }));

const storage = new Map<string, string>();

beforeEach(() => {
  storage.clear();
  vi.stubGlobal('localStorage', {
    getItem: (key: string) => storage.get(key) ?? null,
    setItem: (key: string, value: string) => {
      storage.set(key, value);
    },
    removeItem: (key: string) => {
      storage.delete(key);
    },
    clear: () => {
      storage.clear();
    },
  });
});

describe('Root onboarding', () => {
  it('allows users to dismiss first-run onboarding when the workspace is not configured yet', () => {
    render(<Root />);

    expect(screen.getByRole('heading', { name: 'Join a studio' })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Skip for now' }));

    expect(screen.getByText('My work')).toBeInTheDocument();
    expect(localStorage.getItem('biohazardfs.onboardingDismissed')).toBe('true');
  });

  it('allows users to go backward and finish the preview flow', () => {
    render(<Root />);

    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    expect(screen.getByRole('heading', { name: 'Checking your workstation…' })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('heading', { name: 'Join a studio' })).toBeInTheDocument();

    for (let i = 0; i < 4; i += 1) {
      fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    }
    fireEvent.click(screen.getByRole('button', { name: 'Done' }));

    expect(screen.getByText('My work')).toBeInTheDocument();
  });
});
