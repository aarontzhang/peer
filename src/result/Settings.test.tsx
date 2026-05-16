import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { Settings } from './Settings';
import { emitTauriEvent, invoke, mockCommands } from '@/test/tauriMock';

const installedHotkey = {
  keybind: { kind: 'rightOption' as const },
  label: 'Right Option',
  installed: true,
  reason: null,
};

function renderSettings() {
  const onClose = vi.fn();
  render(<Settings open onClose={onClose} />);
  return { onClose };
}

describe('Settings interactions', () => {
  it('handles mocked sign-in success and sign-out', async () => {
    const user = userEvent.setup();
    let account = { signedIn: false, email: null as string | null };
    mockCommands({
      get_session: () => account,
      get_hotkey_status: () => installedHotkey,
      get_permission_mode: () => 'ask',
      start_google_sign_in: () => 'https://supabase.example/auth',
      sign_out: () => {
        account = { signedIn: false, email: null };
      },
    });

    renderSettings();

    await user.click(await screen.findByRole('button', { name: 'Sign in' }));
    expect(screen.getByText('Waiting for browser sign-in…')).toBeInTheDocument();

    account = { signedIn: true, email: 'user@example.com' };
    await act(async () => {
      emitTauriEvent('auth:changed', { signedIn: true, email: 'user@example.com' });
    });

    expect(await screen.findByText('user@example.com')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Sign out' }));
    expect(await screen.findByRole('button', { name: 'Sign in' })).toBeInTheDocument();
  });

  it('shows the no-account OAuth path', async () => {
    const user = userEvent.setup();
    mockCommands({
      get_session: () => ({ signedIn: false, email: null }),
      get_hotkey_status: () => installedHotkey,
      get_permission_mode: () => 'ask',
      start_google_sign_in: () => 'https://supabase.example/auth',
    });

    renderSettings();

    await user.click(await screen.findByRole('button', { name: 'Sign in' }));
    act(() => {
      emitTauriEvent('auth:changed', { signedIn: false, email: null, reason: 'no_account' });
    });

    expect(await screen.findByText(/you don't have an account yet/i)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: "Creating account is disabled" })).toBeDisabled();
  });

  it('captures a chord shortcut and saves permission mode changes', async () => {
    const user = userEvent.setup();
    mockCommands({
      get_session: () => ({ signedIn: false, email: null }),
      get_hotkey_status: () => installedHotkey,
      get_permission_mode: () => 'ask',
      set_recording_keybind: ({ keybind }) => ({
        keybind,
        label: '⌘+K',
        installed: true,
        reason: null,
      }),
      set_permission_mode: ({ mode }) => mode,
    });
    const { onClose } = renderSettings();

    await user.click(await screen.findByRole('button', { name: 'Change' }));
    fireEvent.keyDown(window, {
      key: 'k',
      code: 'KeyK',
      metaKey: true,
    });

    expect(screen.getByText('⌘')).toBeInTheDocument();
    expect(screen.getByText('K')).toBeInTheDocument();

    await user.click(screen.getByRole('radio', { name: 'Allow everything' }));
    await user.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('set_recording_keybind', {
        keybind: { kind: 'chord', mods: ['super'], code: 'KeyK', label: '⌘+K' },
      });
      expect(invoke).toHaveBeenCalledWith('set_permission_mode', { mode: 'bypass' });
      expect(onClose).toHaveBeenCalled();
    });
  });
});
