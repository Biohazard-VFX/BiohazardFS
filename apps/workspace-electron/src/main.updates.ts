import { app } from 'electron';
import { autoUpdater } from 'electron-updater';

import { type ReleaseChannel } from './main.prefs';

export type UpdateState =
  'idle' | 'checking' | 'available' | 'not_available' | 'unavailable' | 'error';

export type UpdateStatus = {
  state: UpdateState;
  channel: ReleaseChannel;
  currentVersion: string;
  packaged: boolean;
  updateVersion?: string;
  message?: string;
  checkedAt?: string;
};

let lastStatus: UpdateStatus = {
  state: 'idle',
  channel: 'dev',
  currentVersion: app.getVersion(),
  packaged: app.isPackaged,
};

function now(): string {
  return new Date().toISOString();
}

function updateChannel(channel: ReleaseChannel): string {
  return channel === 'stable' ? 'latest' : channel;
}

type ParsedVersion = {
  major: number;
  minor: number;
  patch: number;
  prerelease: string[];
};

function parseVersion(version: string): ParsedVersion | null {
  const [core, prerelease = ''] = version.split('+')[0].split('-', 2);
  const parts = core.split('.').map((part) => Number.parseInt(part, 10));
  if (parts.length < 3 || parts.some((part) => !Number.isFinite(part) || part < 0)) {
    return null;
  }
  return {
    major: parts[0],
    minor: parts[1],
    patch: parts[2],
    prerelease: prerelease ? prerelease.split('.') : [],
  };
}

function comparePrerelease(left: string[], right: string[]): number {
  if (left.length === 0 && right.length === 0) return 0;
  if (left.length === 0) return 1;
  if (right.length === 0) return -1;

  const length = Math.max(left.length, right.length);
  for (let index = 0; index < length; index += 1) {
    const a = left[index];
    const b = right[index];
    if (a === undefined) return -1;
    if (b === undefined) return 1;
    const aNumeric = /^\d+$/.test(a);
    const bNumeric = /^\d+$/.test(b);
    if (aNumeric && bNumeric) {
      const diff = Number(a) - Number(b);
      if (diff !== 0) return Math.sign(diff);
      continue;
    }
    if (aNumeric !== bNumeric) return aNumeric ? -1 : 1;
    const diff = a.localeCompare(b);
    if (diff !== 0) return Math.sign(diff);
  }
  return 0;
}

function isNewerVersion(candidateVersion: string, currentVersion: string): boolean {
  const candidate = parseVersion(candidateVersion);
  const current = parseVersion(currentVersion);
  if (!candidate || !current) return false;

  for (const key of ['major', 'minor', 'patch'] as const) {
    const diff = candidate[key] - current[key];
    if (diff !== 0) return diff > 0;
  }
  return comparePrerelease(candidate.prerelease, current.prerelease) > 0;
}

function baseStatus(channel: ReleaseChannel): UpdateStatus {
  return {
    state: 'idle',
    channel,
    currentVersion: app.getVersion(),
    packaged: app.isPackaged,
  };
}

export function getUpdateStatus(channel: ReleaseChannel): UpdateStatus {
  if (lastStatus.channel !== channel || lastStatus.currentVersion !== app.getVersion()) {
    lastStatus = baseStatus(channel);
  }
  return lastStatus;
}

export function configureAutoUpdater(channel: ReleaseChannel): void {
  autoUpdater.autoDownload = false;
  autoUpdater.autoInstallOnAppQuit = false;
  autoUpdater.allowPrerelease = channel !== 'stable';
  autoUpdater.channel = updateChannel(channel);
}

export async function checkForUpdates(channel: ReleaseChannel): Promise<UpdateStatus> {
  configureAutoUpdater(channel);

  if (!app.isPackaged) {
    lastStatus = {
      ...baseStatus(channel),
      state: 'unavailable',
      message: 'Update checks require a packaged installer build.',
      checkedAt: now(),
    };
    return lastStatus;
  }

  lastStatus = {
    ...baseStatus(channel),
    state: 'checking',
    message: 'Checking for updates…',
    checkedAt: now(),
  };

  try {
    const result = await autoUpdater.checkForUpdates();
    const info = result?.updateInfo;
    if (!info) {
      lastStatus = {
        ...baseStatus(channel),
        state: 'not_available',
        message: 'No update information was returned.',
        checkedAt: now(),
      };
      return lastStatus;
    }

    const available = isNewerVersion(info.version, app.getVersion());
    lastStatus = {
      ...baseStatus(channel),
      state: available ? 'available' : 'not_available',
      updateVersion: info.version,
      message: available
        ? `Version ${info.version} is available. Downloads are manual until installer restart safety is implemented.`
        : 'Biohazard Workspace is up to date.',
      checkedAt: now(),
    };
    return lastStatus;
  } catch (error) {
    lastStatus = {
      ...baseStatus(channel),
      state: 'error',
      message: error instanceof Error ? error.message : String(error),
      checkedAt: now(),
    };
    return lastStatus;
  }
}
