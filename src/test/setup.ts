import '@testing-library/jest-dom/vitest';
import { afterEach, vi } from 'vitest';
import { cleanup } from '@testing-library/react';
import { clipboard, dialog, invoke, listen, resetTauriMocks } from './tauriMock';

vi.mock('@tauri-apps/api/core', () => ({
  invoke,
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen,
}));

vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({
  writeText: clipboard.writeText,
}));

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: dialog.open,
}));

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    outerSize: vi.fn(async () => ({ width: 96, height: 184 })),
    outerPosition: vi.fn(async () => ({ x: 10, y: 20 })),
  }),
  currentMonitor: vi.fn(async () => ({
    scaleFactor: 1,
    position: { x: 0, y: 0 },
    size: { width: 1440, height: 900 },
  })),
}));

Object.defineProperty(window.navigator, 'clipboard', {
  configurable: true,
  value: {
    writeText: vi.fn(async (text: string) => {
      clipboard.text = text;
    }),
  },
});

afterEach(() => {
  cleanup();
  resetTauriMocks();
  window.localStorage.clear();
});
