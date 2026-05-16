# Peer

Screen-recording → Claude-Code instruction set. Tap **Right Option** to start, tap again to stop, while you talk and point. Peer turns the clip into a copy-paste-ready instruction set.

## Architecture

```
Recording keybind (Right Option by default; Fn or a user-chosen chord in Settings)
   └─ Rust orchestrator
       ├─ Swift sidecar (ScreenCaptureKit) → mp4   [ffmpeg avfoundation fallback]
       ├─ ffmpeg scene-detect keyframes (uniform-fps fallback)
       ├─ ffmpeg → mp3 → parallel transcription chunks → dedupe
       └─ parallel vision window analyzers → aggregator → markdown stream
```

Two windows: a 56×144 always-on-top **pill** (`pill.html`) and a 760×560 **result window** (`index.html`).

## Develop

```sh
pnpm install
pnpm tauri:dev
```

`tauri:dev` builds the Swift sidecar and starts Tauri. The desktop app talks to the managed Vercel backend (`https://peer-wheat.vercel.app`) for all model calls — no local API keys are required. Sign in to a Peer account on first launch.

The first run pulls Tauri 2 deps and compiles the Rust core (~60s the first time). Once the pill window appears, tap **Right Option** to start a recording, tap **Right Option** again to stop. You can switch the recording keybind (Right Option / Fn / a custom chord) in Settings.

### Swift sidecar

`pnpm tauri:dev` runs `pnpm sidecar` for you. The standalone script is:

```sh
pnpm sidecar
```

If the sidecar binary is missing at runtime, capture falls back to `ffmpeg avfoundation` automatically — useful when iterating without Xcode available. Production builds always use the sidecar for better cursor rendering and lower CPU.

This requires **full Xcode** (not just Command Line Tools) so that `xcrun --sdk macosx --show-sdk-platform-path` resolves. If `swift build` errors with `unable to lookup item 'PlatformPath'`, install Xcode from the App Store and run `sudo xcode-select -s /Applications/Xcode.app`.

All model calls — dev and production — route through the managed Vercel backend using a Peer account token stored in macOS Keychain. There is no local-keys fallback.

### Managed backend

The Vercel API surface lives in `api/` and uses Supabase tables from `supabase/schema.sql`.

Required Vercel environment:

```sh
OPENAI_API_KEY=...
ANTHROPIC_API_KEY=...
SUPABASE_URL=...
SUPABASE_SERVICE_ROLE_KEY=...
PEER_FREE_BETA_MONTHLY_LIMIT=25   # optional; invalid/empty values fall back to 25
PEER_MACOS_DOWNLOAD_URL=...   # optional; falls back to https://github.com/aarontzhang/peer/releases/latest/download/Peer.dmg

# Optional model overrides
PEER_WINDOW_MODEL=...
PEER_AGGREGATOR_MODEL=...
PEER_TITLE_MODEL=...
```

The public download/account site lives in `site/`. Sign-in is Google OAuth via Supabase implicit flow: the desktop app opens `${SUPABASE_URL}/auth/v1/authorize?provider=google&redirect_to=http://127.0.0.1:17643/auth-callback`, the local callback returns the browser fragment to the running app, and the session lands in macOS Keychain. Supabase Auth redirect URLs must include `http://127.0.0.1:17643/auth-callback`; keep the hosted `/api/auth-callback` URLs allowed for old builds that still deep-link back to `peer://auth#access_token=…`. Open the Supabase dashboard → Authentication → Sign In/Up to toggle whether new users can self-serve sign up.

Recording-generation endpoints enforce `PEER_FREE_BETA_MONTHLY_LIMIT` completed recordings per Supabase user per UTC calendar month. Over-limit users receive `429` with `{ "error": "monthly beta recording limit reached" }` before new provider calls are made.

### macOS release

```sh
APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)" \
APPLE_NOTARYTOOL_PROFILE="peer-notary" \
pnpm release:mac
```

The release script builds the Swift sidecar, builds the Tauri macOS app, removes LaunchServices keys that block modern macOS launch, signs with the hardened runtime, creates a DMG, signs the DMG, and notarizes/staples it. Public releases require a Developer ID Application identity and a notarytool profile so the downloadable DMG opens normally for first-time users.

For local packaging smoke tests only:

```sh
PEER_ALLOW_UNSIGNED_RELEASE=1 pnpm release:mac
```

That path creates an ad-hoc signed DMG for launch testing, but Gatekeeper will reject it on other Macs. Do not upload it as the public website download.

Published DMG smoke check:

```sh
curl -sIL https://www.peercv.com/download/latest
curl -L https://www.peercv.com/download/latest -o Peer.dmg
codesign -dv --verbose=4 Peer.dmg
spctl -a -t open --context context:primary-signature -vv Peer.dmg
hdiutil attach Peer.dmg
defaults read /Volumes/Peer/Peer.app/Contents/Info CFBundleIdentifier
hdiutil detach /Volumes/Peer
```

`spctl` must accept the DMG, and the bundle identifier must be `com.aaronzhang.peer`.

## Layout

```
src/                        # React 19 frontend
  pill/                     # 56×144 ambient pill (entry: pill.html)
  result/                   # main result window (entry: index.html)
  lib/                      # ipc + global key hooks
  styles/tokens.css         # Apple-tuned design tokens (Tailwind v4 @theme)
src-tauri/                  # Rust core
  src/recording/            # capture lifecycle, Swift sidecar driver
  src/hotkey/fn_tap.rs      # CGEventTap modifier-tap detector
  src/pipeline/             # keyframes, transcribe, analyze, prompts, ffprobe
  src/db/                   # sqlx schema for recordings + results
  src/ipc.rs                # Tauri commands
capture-sidecar/            # Swift package — ScreenCaptureKit → mp4
```

## Permissions

First run will prompt for **Screen Recording**, **Microphone**, and **Accessibility** (the last one is required for modifier-tap keybinds like Fn and Right Option). All three need to be granted in System Settings → Privacy & Security.

### TCC reset (only if you've been running older builds)

If you ever ran a pre-stable-signing build of Peer, macOS may have cached a TCC entry against an old code signature. Symptoms: the screen-recording prompt fires every time you press record even though Peer is toggled on in System Settings. To clear it:

1. Open *System Settings → Privacy & Security → Screen & System Audio Recording* and *Microphone*. If `Peer` (or `PeerCapture`) is listed, select it and click **−** to remove it.
2. From a terminal:
   ```sh
   tccutil reset ScreenCapture dev.aaronzhang.peer
   tccutil reset Microphone dev.aaronzhang.peer
   tccutil reset Microphone dev.aaronzhang.peer.capture
   ```
3. Quit Peer, run `pnpm tauri:dev` again, and grant fresh on the next prompt. The grant now sticks across rebuilds because the dev binary is re-signed with stable identifier `dev.aaronzhang.peer` (see `src-tauri/.cargo/config.toml` and `src-tauri/bin/dev-runner.sh`).
