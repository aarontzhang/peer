# Hummingbird

Screen-recording → Claude-Code instruction set. Tap **Fn** (Globe) to start, tap again to stop, while you talk and point. Hummingbird turns the clip into a copy-paste-ready instruction set in <12 seconds.

## Architecture

```
Fn-tap (CGEventTap)
   └─ Rust orchestrator
       ├─ Swift sidecar (ScreenCaptureKit) → mp4
       ├─ ffmpeg scene-detect keyframes (uniform-fps fallback)
       ├─ ffmpeg → mp3 → parallel Whisper chunks → dedupe
       └─ parallel Claude window analyzers → aggregator → markdown stream
```

Two windows: a 280×40 always-on-top **pill** (`pill.html`) and a 760×560 **result window** (`index.html`).

## Develop

```sh
pnpm install
pnpm tauri dev
```

The first run pulls Tauri 2 deps and compiles the Rust core (~60s the first time). Once the pill window appears, tap **Fn** to start a recording, tap **Fn** again to stop.

### Swift sidecar (optional, for production capture quality)

In dev mode, capture falls back to `ffmpeg avfoundation` automatically — no Swift build needed. For final builds (better cursor rendering, lower CPU), build the ScreenCaptureKit sidecar:

```sh
pnpm sidecar
```

This requires **full Xcode** (not just Command Line Tools) so that `xcrun --sdk macosx --show-sdk-platform-path` resolves. If `swift build` errors with `unable to lookup item 'PlatformPath'`, install Xcode from the App Store and run `sudo xcode-select -s /Applications/Xcode.app`.

API keys (OpenAI for Whisper, Anthropic for Claude) live in macOS Keychain. Add them via the Settings panel inside the result window.

## Layout

```
src/                        # React 19 frontend
  pill/                     # 280×40 ambient pill (entry: pill.html)
  result/                   # main result window (entry: index.html)
  lib/                      # ipc + global key hooks
  styles/tokens.css         # Apple-tuned design tokens (Tailwind v4 @theme)
src-tauri/                  # Rust core
  src/recording/            # capture lifecycle, Swift sidecar driver, cursor toggle
  src/hotkey/fn_tap.rs      # CGEventTap Fn-tap detector
  src/pipeline/             # keyframes, transcribe, analyze, prompts, ffprobe
  src/db/                   # sqlx schema for recordings + results
  src/ipc.rs                # Tauri commands
capture-sidecar/            # Swift package — ScreenCaptureKit → mp4
```

## Permissions

First run will prompt for **Screen Recording**, **Microphone**, and **Accessibility** (the last one is required for the global Fn-tap detector). All three need to be granted in System Settings → Privacy & Security.
