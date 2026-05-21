import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { FabMenu } from './FabMenu';
import { dialog, invoke, mockCommands } from '@/test/tauriMock';

function setup(busy = false) {
  const onUploadStarted = () => {};
  const errors: string[] = [];
  render(
    <FabMenu
      busy={busy}
      onUploadStarted={onUploadStarted}
      onUploadError={(m) => errors.push(m)}
    />,
  );
  return { errors };
}

describe('FabMenu', () => {
  it('opens the popover and triggers upload_recording with the picked path', async () => {
    const user = userEvent.setup();
    mockCommands({
      upload_recording: ({ sourcePath }) => {
        expect(sourcePath).toBe('/Users/me/clip.mp4');
        return 'new-rec-id';
      },
    });
    dialog.nextPick = '/Users/me/clip.mp4';

    setup();

    await user.click(screen.getByRole('button', { name: 'Add to Peer' }));
    await user.click(screen.getByRole('menuitem', { name: /Upload video/ }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('upload_recording', {
        sourcePath: '/Users/me/clip.mp4',
      });
    });
  });

  it('disables the upload option while the app is busy', async () => {
    const user = userEvent.setup();
    setup(true);

    await user.click(screen.getByRole('button', { name: 'Add to Peer' }));
    const item = screen.getByRole('menuitem', { name: /Upload video/ });
    expect(item).toBeDisabled();
    expect(item).toHaveAttribute(
      'title',
      'Wait for the current recording to finish',
    );
  });

  it('surfaces a Rust error to onUploadError', async () => {
    const user = userEvent.setup();
    mockCommands({
      upload_recording: () => {
        throw new Error('Sign in to use Peer — uploads require a Peer account');
      },
    });
    dialog.nextPick = '/Users/me/clip.mp4';

    const { errors } = setup();

    await user.click(screen.getByRole('button', { name: 'Add to Peer' }));
    await user.click(screen.getByRole('menuitem', { name: /Upload video/ }));

    await waitFor(() => {
      expect(errors).toEqual([
        'Sign in to use Peer — uploads require a Peer account',
      ]);
    });
  });

  it('no-ops when the user cancels the file picker', async () => {
    const user = userEvent.setup();
    mockCommands({});
    dialog.nextPick = null;

    setup();

    await user.click(screen.getByRole('button', { name: 'Add to Peer' }));
    await user.click(screen.getByRole('menuitem', { name: /Upload video/ }));

    await waitFor(() => {
      expect(dialog.open).toHaveBeenCalledTimes(1);
    });
    expect(invoke).not.toHaveBeenCalled();
  });
});
