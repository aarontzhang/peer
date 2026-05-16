import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { Pill } from './Pill';
import { emitTauriEvent, invoke, mockCommands } from '@/test/tauriMock';

describe('Pill interactions', () => {
  it('starts from idle and stops while recording', async () => {
    const user = userEvent.setup();
    mockCommands({
      start_recording: () => 'rec-1',
      stop_recording: () => undefined,
      cursor_position: () => [0, 0],
    });

    render(<Pill />);
    await user.click(screen.getByRole('button', { name: 'Start recording' }));

    expect(invoke).toHaveBeenCalledWith('start_recording');

    act(() => {
      emitTauriEvent('pill:state', { kind: 'recording', id: 'rec-1', elapsedMs: 1400 });
    });
    await user.click(screen.getByRole('button', { name: 'Stop recording' }));

    expect(invoke).toHaveBeenCalledWith('stop_recording');
  });

  it('sends or discards a stopped recording by button and keyboard', async () => {
    const user = userEvent.setup();
    mockCommands({
      send_recording: () => undefined,
      cancel_recording: () => undefined,
      cursor_position: () => [0, 0],
    });

    render(<Pill />);
    act(() => {
      emitTauriEvent('pill:state', { kind: 'stopped', id: 'rec-1', durationMs: 5100 });
    });

    await user.click(screen.getByRole('button', { name: 'Send recording for processing' }));
    expect(invoke).toHaveBeenCalledWith('send_recording');

    fireEvent.keyDown(window, { key: 'Backspace' });
    expect(invoke).toHaveBeenCalledWith('cancel_recording');
  });
});
