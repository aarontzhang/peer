import { vi } from 'vitest';

type CommandHandler = (args: Record<string, unknown>) => unknown | Promise<unknown>;
type EventHandler<T = unknown> = (event: { event: string; payload: T }) => void;

const commandHandlers = new Map<string, CommandHandler>();
const eventHandlers = new Map<string, Set<EventHandler>>();

export const clipboard = {
  text: '',
  writeText: vi.fn(async (text: string) => {
    clipboard.text = text;
  }),
};

export const dialog = {
  nextPick: null as string | null,
  open: vi.fn(async () => dialog.nextPick),
};

export const invoke = vi.fn(async (command: string, args?: Record<string, unknown>) => {
  const handler = commandHandlers.get(command);
  if (!handler) {
    throw new Error(`No invoke mock registered for ${command}`);
  }
  return handler(args ?? {});
});

export const listen = vi.fn(async <T>(event: string, handler: EventHandler<T>) => {
  const handlers = eventHandlers.get(event) ?? new Set<EventHandler>();
  handlers.add(handler as EventHandler);
  eventHandlers.set(event, handlers);
  return () => {
    handlers.delete(handler as EventHandler);
  };
});

export function mockCommand(command: string, handler: CommandHandler) {
  commandHandlers.set(command, handler);
}

export function mockCommands(handlers: Record<string, CommandHandler>) {
  for (const [command, handler] of Object.entries(handlers)) {
    mockCommand(command, handler);
  }
}

export function emitTauriEvent<T>(event: string, payload: T) {
  for (const handler of eventHandlers.get(event) ?? []) {
    handler({ event, payload });
  }
}

export function resetTauriMocks() {
  commandHandlers.clear();
  eventHandlers.clear();
  invoke.mockClear();
  listen.mockClear();
  clipboard.text = '';
  clipboard.writeText.mockClear();
  dialog.nextPick = null;
  dialog.open.mockClear();
}

export function defaultRecording(overrides: Partial<{
  id: string;
  createdAt: string;
  durationMs: number;
  videoPath: string;
  status: 'recording' | 'stopped' | 'processing' | 'done' | 'failed' | 'canceled';
  summary: string | null;
  body: string | null;
  transcript: string | null;
  thinking: string | null;
  error: string | null;
}> = {}) {
  return {
    id: 'rec-1',
    createdAt: '2026-05-15T12:00:00Z',
    durationMs: 12_000,
    videoPath: '/tmp/rec-1.mp4',
    status: 'done' as const,
    summary: 'Fix button state',
    body: 'Click the save button and verify state.',
    transcript: 'Please fix the button state.',
    thinking: null,
    error: null,
    ...overrides,
  };
}
