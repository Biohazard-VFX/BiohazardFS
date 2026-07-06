import { app, BrowserWindow, Menu, type MenuItemConstructorOptions } from 'electron';

import { ZOOM_MAX, ZOOM_MIN, ZOOM_STEP, loadPrefs, savePrefs } from './main.prefs';

function broadcastPrefs(prefs: { windowChrome: string; zoomFactor: number }) {
  for (const win of BrowserWindow.getAllWindows()) {
    win.webContents.send('prefs:changed', prefs);
  }
}

// Application menu. Carries the standard Edit roles (so copy/paste/quit work
// everywhere, including macOS) plus a View menu whose zoom items drive the
// persisted webContents zoom factor. Accelerators use CmdOrCtrl so they are
// Ctrl on Linux/Windows and Cmd on macOS.

function focusedWebContents() {
  const win = BrowserWindow.getFocusedWindow();
  return win?.webContents ?? undefined;
}

function applyZoom(next: number) {
  const prefs = savePrefs({ zoomFactor: next });
  for (const win of BrowserWindow.getAllWindows()) {
    win.webContents.setZoomFactor(next);
  }
  broadcastPrefs(prefs);
}

function zoomIn() {
  const current = focusedWebContents()?.getZoomFactor() ?? loadPrefs().zoomFactor;
  applyZoom(Math.min(ZOOM_MAX, Math.round((current + ZOOM_STEP) * 100) / 100));
}

function zoomOut() {
  const current = focusedWebContents()?.getZoomFactor() ?? loadPrefs().zoomFactor;
  applyZoom(Math.max(ZOOM_MIN, Math.round((current - ZOOM_STEP) * 100) / 100));
}

function zoomReset() {
  applyZoom(1);
}

export function buildAppMenu(): Menu {
  const isMac = process.platform === 'darwin';

  const template: MenuItemConstructorOptions[] = [
    ...(isMac
      ? ([
          {
            label: app.name,
            submenu: [
              { role: 'about' },
              { type: 'separator' },
              { role: 'services' },
              { type: 'separator' },
              { role: 'hide' },
              { role: 'hideOthers' },
              { role: 'unhide' },
              { type: 'separator' },
              { role: 'quit' },
            ],
          },
        ] as MenuItemConstructorOptions[])
      : []),
    {
      label: 'File',
      submenu: [isMac ? { role: 'close' } : { role: 'quit' }],
    },
    {
      label: 'Edit',
      submenu: [
        { role: 'undo' },
        { role: 'redo' },
        { type: 'separator' },
        { role: 'cut' },
        { role: 'copy' },
        { role: 'paste' },
        { role: 'selectAll' },
      ],
    },
    {
      label: 'View',
      submenu: [
        { role: 'reload' },
        { role: 'forceReload' },
        { type: 'separator' },
        {
          label: 'Zoom In',
          accelerator: 'CmdOrCtrl+=',
          click: () => zoomIn(),
        },
        {
          label: 'Zoom Out',
          accelerator: 'CmdOrCtrl+-',
          click: () => zoomOut(),
        },
        {
          label: 'Actual Size',
          accelerator: 'CmdOrCtrl+0',
          click: () => zoomReset(),
        },
        { type: 'separator' },
        { role: 'toggleDevTools' },
      ],
    },
    {
      label: 'Window',
      submenu: [{ role: 'minimize' }, { role: 'zoom' }, { type: 'separator' }, { role: 'front' }],
    },
  ];

  return Menu.buildFromTemplate(template);
}
