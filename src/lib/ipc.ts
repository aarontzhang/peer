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

export type ThinkingEvent = { id: string; thinking: string };

export type AccountStatus = {
  signedIn: boolean;
  email: string | null;
};

export type AuthChangedPayload = AccountStatus & {
  error?: string;
  reason?: 'no_account';
};

export type RecordingKeybind =
  | { kind: 'fn' }
  | { kind: 'rightOption' }
  | { kind: 'chord'; mods: string[]; code: string; label: string };

export type HotkeyStatus = {
  keybind: RecordingKeybind;
  label: string;
  installed: boolean;
  reason: string | null;
};

export type PermissionMode = 'ask' | 'bypass';

export const ipc = {
  startRecording: () => invoke<string>('start_recording'),
  stopRecording: () => invoke<void>('stop_recording'),
  cancelRecording: () => invoke<void>('cancel_recording'),
  sendRecording: () => invoke<void>('send_recording'),
  retryRecording: (id: string) => invoke<void>('retry_recording', { id }),
  uploadRecording: (sourcePath: string) =>
    invoke<string>('upload_recording', { sourcePath }),
  listRecordings: () => invoke<Recording[]>('list_recordings'),
  getRecording: (id: string) => invoke<Recording | null>('get_recording', { id }),
  deleteRecording: (id: string) => invoke<void>('delete_recording', { id }),
  openResultWindow: () => invoke<void>('open_result_window'),
  movePill: (x: number, y: number) => invoke<void>('move_pill', { x, y }),
  cursorPosition: () => invoke<[number, number]>('cursor_position'),
  getSession: () => invoke<AccountStatus>('get_session'),
  startGoogleSignIn: () => invoke<string>('start_google_sign_in'),
  signOut: () => invoke<void>('sign_out'),
  getHotkeyStatus: () => invoke<HotkeyStatus>('get_hotkey_status'),
  setRecordingKeybind: (keybind: RecordingKeybind) =>
    invoke<HotkeyStatus>('set_recording_keybind', { keybind }),
  getPermissionMode: () => invoke<PermissionMode>('get_permission_mode'),
  setPermissionMode: (mode: PermissionMode) =>
    invoke<PermissionMode>('set_permission_mode', { mode }),

  onPillEvent: (cb: (e: PillEvent) => void): Promise<UnlistenFn> =>
    listen<PillEvent>('pill:state', (e) => cb(e.payload)),
  onResultChunk: (cb: (c: ResultChunk) => void): Promise<UnlistenFn> =>
    listen<ResultChunk>('result:chunk', (e) => cb(e.payload)),
  onThinking: (cb: (t: ThinkingEvent) => void): Promise<UnlistenFn> =>
    listen<ThinkingEvent>('result:thinking', (e) => cb(e.payload)),
  onHotkeyStatus: (cb: (s: HotkeyStatus) => void): Promise<UnlistenFn> =>
    listen<HotkeyStatus>('hotkey:status', (e) => cb(e.payload)),
  onAuthChanged: (cb: (s: AuthChangedPayload) => void): Promise<UnlistenFn> =>
    listen<AuthChangedPayload>('auth:changed', (e) => cb(e.payload)),
};

/**
 * Errors thrown from the retry IPC are stringified `anyhow` Display output.
 * The Rust side prefixes a stable sentinel when the on-disk video is gone so
 * the UI can disable the button instead of throwing a generic alert.
 */
export function isVideoMissingError(err: unknown): boolean {
  const msg = err instanceof Error ? err.message : String(err ?? '');
  return msg.includes('VIDEO_MISSING');
}

export function formatDuration(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}

