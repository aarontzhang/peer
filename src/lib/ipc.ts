import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type RecordingStatus = 'recording' | 'stopped' | 'processing' | 'done' | 'failed' | 'canceled';

export type Recording = {
  id: string;
  createdAt: string;
  durationMs: number;
  videoPath: string;
  status: RecordingStatus;
  summary: string | null;
  body: string | null;
  transcript: string | null;
  thinking: string | null;
  error: string | null;
};

export type PillEvent =
  | { kind: 'idle' }
  | { kind: 'recording'; id: string; elapsedMs: number }
  | { kind: 'stopped'; id: string; durationMs: number }
  | { kind: 'processing'; id: string; label: string; progress: number }
  | { kind: 'done'; id: string }
  | { kind: 'error'; id: string | null; message: string };

export type ResultChunk = {
  id: string;
  kind: 'begin' | 'delta' | 'end';
  text: string;
};

export type TranscriptEvent = { id: string; transcript: string };

export type ThinkingEvent = { id: string; thinking: string };

export type ApiKeyStatus = { openai: boolean; anthropic: boolean };

export type AccountStatus = {
  signedIn: boolean;
  backendUrl: string;
  deviceId: string;
};

export type RecordingKeybind = 'rightOption' | 'fn' | 'cmdShiftR';

export type HotkeyStatus = {
  keybind: RecordingKeybind;
  label: string;
  installed: boolean;
  reason: string | null;
};

export const ipc = {
  startRecording: () => invoke<string>('start_recording'),
  stopRecording: () => invoke<void>('stop_recording'),
  cancelRecording: () => invoke<void>('cancel_recording'),
  sendRecording: () => invoke<void>('send_recording'),
  listRecordings: () => invoke<Recording[]>('list_recordings'),
  getRecording: (id: string) => invoke<Recording | null>('get_recording', { id }),
  deleteRecording: (id: string) => invoke<void>('delete_recording', { id }),
  openResultWindow: () => invoke<void>('open_result_window'),
  movePill: (x: number, y: number) => invoke<void>('move_pill', { x, y }),
  cursorPosition: () => invoke<[number, number]>('cursor_position'),
  setApiKey: (provider: 'openai' | 'anthropic', key: string) =>
    invoke<void>('set_api_key', { args: { provider, key } }),
  getApiKeyStatus: () => invoke<ApiKeyStatus>('get_api_key_status'),
  getAccountStatus: () => invoke<AccountStatus>('get_account_status'),
  openAccountLogin: () => invoke<string>('open_account_login'),
  setDeviceToken: (token: string) => invoke<void>('set_device_token', { args: { token } }),
  signOut: () => invoke<void>('sign_out'),
  getHotkeyStatus: () => invoke<HotkeyStatus>('get_hotkey_status'),
  setRecordingKeybind: (keybind: RecordingKeybind) =>
    invoke<HotkeyStatus>('set_recording_keybind', { keybind }),

  onPillEvent: (cb: (e: PillEvent) => void): Promise<UnlistenFn> =>
    listen<PillEvent>('pill:state', (e) => cb(e.payload)),
  onResultChunk: (cb: (c: ResultChunk) => void): Promise<UnlistenFn> =>
    listen<ResultChunk>('result:chunk', (e) => cb(e.payload)),
  onTranscript: (cb: (t: TranscriptEvent) => void): Promise<UnlistenFn> =>
    listen<TranscriptEvent>('result:transcript', (e) => cb(e.payload)),
  onThinking: (cb: (t: ThinkingEvent) => void): Promise<UnlistenFn> =>
    listen<ThinkingEvent>('result:thinking', (e) => cb(e.payload)),
  onHotkeyStatus: (cb: (s: HotkeyStatus) => void): Promise<UnlistenFn> =>
    listen<HotkeyStatus>('hotkey:status', (e) => cb(e.payload)),
};

export function formatDuration(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}

export function formatRelative(iso: string): string {
  const t = new Date(iso).getTime();
  const diff = Date.now() - t;
  const min = Math.floor(diff / 60_000);
  if (min < 1) return 'Just now';
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const d = Math.floor(hr / 24);
  if (d < 7) return `${d}d ago`;
  return new Date(iso).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
}
