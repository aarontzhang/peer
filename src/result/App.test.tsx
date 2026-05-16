import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { App } from './App';
import {
  clipboard,
  defaultRecording,
  emitTauriEvent,
  invoke,
  mockCommands,
} from '@/test/tauriMock';

const installedHotkey = {
  keybind: { kind: 'rightOption' as const },
  label: 'Right Option',
  installed: true,
  reason: null,
};

function mockAppCommands(recordings = [defaultRecording()]) {
  let rows = recordings;
  mockCommands({
    list_recordings: () => rows,
    get_hotkey_status: () => installedHotkey,
    get_session: () => ({ signedIn: false, email: null }),
    get_permission_mode: () => 'ask',
    retry_recording: () => undefined,
    delete_recording: ({ id }) => {
      rows = rows.filter((row) => row.id !== id);
    },
  });
  return {
    setRows(next: typeof rows) {
      rows = next;
    },
  };
}

describe('Result app interactions', () => {
  it('saves recordings into the Saved tab and opens/closes detail view', async () => {
    const user = userEvent.setup();
    mockAppCommands();

    render(<App />);

    await screen.findByText('Fix button state');
    await user.click(screen.getByRole('button', { name: 'Save' }));
    await user.click(screen.getByRole('tab', { name: /Saved/ }));

    expect(screen.getByText('Fix button state')).toBeInTheDocument();

    await user.click(screen.getByText('Fix button state'));
    expect(screen.getByRole('dialog', { name: 'Recording detail' })).toBeInTheDocument();
    expect(screen.getByText('Click the save button and verify state.')).toBeInTheDocument();

    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog', { name: 'Recording detail' })).not.toBeInTheDocument();
  });

  it('copies, deletes, and refreshes a recording from the card actions menu', async () => {
    const user = userEvent.setup();
    mockAppCommands();

    render(<App />);

    await screen.findByText('Fix button state');
    await user.click(screen.getByRole('button', { name: 'Show actions' }));
    await user.click(screen.getByRole('button', { name: 'Copy' }));

    expect(clipboard.writeText).toHaveBeenCalledWith('Click the save button and verify state.');

    await user.click(screen.getByRole('button', { name: 'Delete recording' }));
    const dialog = screen.getByRole('dialog', { name: /Delete recording/ });
    await user.click(within(dialog).getByRole('button', { name: 'Delete' }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('delete_recording', { id: 'rec-1' });
    });
    expect(screen.getByText(/Show, don't tell/)).toBeInTheDocument();
  });

  it('streams live result chunks into detail view and copies final output', async () => {
    const user = userEvent.setup();
    const rec = defaultRecording({
      id: 'live-1',
      status: 'processing',
      summary: null,
      body: null,
    });
    mockAppCommands([rec]);

    render(<App />);

    await screen.findByText('Analyzing…');
    await user.click(screen.getByText('Analyzing…'));
    expect(screen.getByText('Writing the refined prompt…')).toBeInTheDocument();

    act(() => {
      emitTauriEvent('result:chunk', { id: 'live-1', kind: 'begin', text: '' });
      emitTauriEvent('result:chunk', { id: 'live-1', kind: 'delta', text: 'Do this ' });
      emitTauriEvent('result:chunk', { id: 'live-1', kind: 'delta', text: 'now.' });
    });

    await screen.findByText('Do this now.');

    act(() => {
      emitTauriEvent('result:chunk', { id: 'live-1', kind: 'end', text: 'Final prompt' });
    });

    await waitFor(() => {
      expect(clipboard.writeText).toHaveBeenCalledWith('Final prompt');
    });
  });

  it('shows hotkey warnings only when no detail overlay is open', async () => {
    const user = userEvent.setup();
    mockAppCommands();
    mockCommands({
      ...Object.fromEntries([]),
      list_recordings: () => [defaultRecording()],
      get_hotkey_status: () => ({
        ...installedHotkey,
        installed: false,
        reason: 'Accessibility is disabled.',
      }),
      get_session: () => ({ signedIn: false, email: null }),
      get_permission_mode: () => 'ask',
    });

    render(<App />);

    expect(await screen.findByText(/Accessibility is disabled/)).toBeInTheDocument();
    await user.click(screen.getByText('Fix button state'));
    expect(screen.queryByText(/Accessibility is disabled/)).not.toBeInTheDocument();
  });

  it('retries canceled recordings from the actions menu', async () => {
    const user = userEvent.setup();
    mockAppCommands([
      defaultRecording({
        status: 'canceled',
        summary: null,
        body: null,
      }),
    ]);

    render(<App />);

    await screen.findByText('Cancelled');
    await user.click(screen.getByRole('button', { name: 'Show actions' }));
    await user.click(screen.getByRole('button', { name: 'Analyze the video' }));

    expect(invoke).toHaveBeenCalledWith('retry_recording', { id: 'rec-1' });
  });
});
