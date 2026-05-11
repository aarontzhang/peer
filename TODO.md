# TODO — Ship Peer beta to a first external tester

Owner: Aaron. Tasks here require credentials, a browser, or human judgement Claude can't do.
The corresponding Claude-side work is tracked in `/Users/aaronzhang/.claude/plans/ok-i-want-you-whimsical-manatee.md`.

Production URL: **https://www.peercv.com**
GitHub repo: **https://github.com/aarontzhang/peer** (already public)

---

## ✅ Already verified by Claude (no action needed)

These are done — listed so you know the current state.

- [x] **Vercel project `peer` is live** at `www.peercv.com`. `/` returns 200, `/download/latest` returns a clean 302 to GitHub Releases.
- [x] **All 7 required env vars** are set on Vercel production (OPENAI_API_KEY, ANTHROPIC_API_KEY, SUPABASE_URL, SUPABASE_SERVICE_ROLE_KEY, SUPABASE_ANON_KEY, SUPABASE_JWT_SECRET, PEER_BETA_INVITE_CODE, PEER_FREE_BETA_MONTHLY_LIMIT, PEER_BACKEND_URL). `PEER_MACOS_DOWNLOAD_URL` is not set — that's fine; `api/download-latest.js` has the correct default baked in.
- [x] **Supabase auth callback** (`/api/auth-callback`) returns 200 and contains the `peer://auth` deep-link logic.
- [x] **The single missing piece** is the GitHub release artifact — `https://github.com/aarontzhang/peer/releases/latest/download/Peer.dmg` returns 404 today because no release exists yet. Tasks 1–3 below fix that.

---

## 1. Publish the v0.2.0 GitHub release  ⏱ ~3 minutes

After Claude finishes rebuilding the DMG (see "Status check" below), you need to publish it as a GitHub release so `/download/latest` resolves.

### Easiest path — let Claude run `gh release create`
Just say: **"go ahead and publish the release."**
Claude will run:
```sh
gh release create v0.2.0 \
  src-tauri/target/release/bundle/dmg/Peer.dmg \
  --title "Peer 0.2.0 (beta)" \
  --notes-file <( echo "First public beta. macOS Apple Silicon only.\n\nFirst-time install: open the DMG, drag Peer to Applications, right-click Peer → Open. macOS will ask once; click Open again." ) \
  --prerelease
```

### Manual path (if you prefer the UI)
1. Open https://github.com/aarontzhang/peer/releases/new
2. **Choose a tag** → type `v0.2.0` → "Create new tag: v0.2.0 on publish"
3. **Title:** `Peer 0.2.0 (beta)`
4. **Description:**
   ```
   First public beta. macOS Apple Silicon only.

   First-time install: open the DMG, drag Peer to Applications,
   right-click Peer → Open. macOS will ask once; click Open again.
   ```
5. **Attach the DMG:** drag the file from `src-tauri/target/release/bundle/dmg/Peer.dmg` into the assets box.
   ⚠️ The asset **must be named exactly `Peer.dmg`** — without the version suffix — because `api/download-latest.js` redirects to that exact filename. Claude will rename it before you upload.
6. Tick **"Set as a pre-release"**.
7. Click **Publish release**.

### How to confirm it worked
After publishing, in a terminal:
```sh
curl -sIL https://www.peercv.com/download/latest | head -25
```
You want to see `HTTP/2 302 → HTTP/2 302 → HTTP/2 200` with `content-type: application/octet-stream` and a `content-length` of roughly 4–8 MB. If the last hop is 404, the asset name on the release page isn't `Peer.dmg` — rename it in the release UI.

---

## 2. Generate + share the beta invite code  ⏱ ~2 minutes

The desktop app gates sign-in on `PEER_BETA_INVITE_CODE`. You already have a value set in Vercel.

### Verify what's currently set
```sh
cd /Users/aaronzhang/Desktop/Peer
vercel env pull .env.vercel.production --environment=production
grep PEER_BETA_INVITE_CODE .env.vercel.production
# then delete it so it doesn't get committed:
rm .env.vercel.production
```

### If you want to change it
1. Go to https://vercel.com/aaronzhangper-gmailcoms-projects/peer/settings/environment-variables
2. Find `PEER_BETA_INVITE_CODE`, click **Edit**, set a new value (any short string — e.g. `peer-beta-2026`).
3. After saving, you must **redeploy** for the new value to take effect: in the Vercel dashboard, **Deployments → … → Redeploy**. Or push any commit.

### Share with the tester
Send your tester:
- The link: **https://www.peercv.com**
- The invite code (whatever string above)
- A short note: *"Click Download. After it downloads, open the DMG, drag Peer into Applications, then right-click Peer → Open (macOS will ask once because the beta isn't notarized — it's safe). When Peer launches, sign in and enter this invite code."*

---

## 3. Smoke-test it yourself before sharing  ⏱ ~10 minutes  (HIGHLY RECOMMENDED)

Don't send the link to an external tester until you've done one full clean-room install. Easiest way: do this on a Mac that isn't your dev machine, **or** delete your local `Peer.app` from `/Applications` first and pretend you're a new user.

### Steps
1. Open **https://www.peercv.com** in a fresh browser tab.
2. Click **Download for macOS** → DMG downloads.
3. Open the DMG in Finder, drag `Peer.app` into Applications.
4. Open Applications → **right-click `Peer.app` → Open** → confirm at the Gatekeeper dialog.
5. macOS will prompt for three permissions in System Settings → Privacy & Security. Grant all three:
   - **Screen & System Audio Recording**
   - **Microphone**
   - **Accessibility** (required for the Fn key-tap detection)
6. When the pill window appears, click the sign-in button. A browser tab opens to your Supabase auth flow → enter your invite code → after sign-in you should be deep-linked back to `peer://auth`. Token gets stored in macOS Keychain.
7. Press **Fn** to start recording → narrate a 5-second screen task → press **Fn** again to stop.
8. The result window should populate with markdown within ~12 seconds.

### If sign-in fails
- The README mentions an `/api/desktop-login` endpoint that doesn't exist in the deployed code (only `/api/auth-callback` exists). Verify that the desktop app's sign-in button opens a Supabase URL (not `/api/desktop-login`). If it's hitting `/api/desktop-login`, that's a code bug in the Rust auth handler — flag it to Claude. (Today, sign-in from your dev build presumably works, so this is more about awareness than a known bug.)
- If you see a Keychain prompt asking to allow Peer to access an item, click **Always Allow**.

### If a recording fails
- Check `~/Library/Logs/Peer/` for Rust logs.
- Common cause: Screen Recording permission was granted to an old build's code signature. Run the TCC reset block from `README.md` ("TCC reset" section).

---

## 4. (Optional but valuable later) Things to consider for the next iteration

Not blocking the current ship, but worth tracking:

- **Intel Mac users will be locked out.** The current DMG is `arm64` only. ~50% of macOS active-user base is still Intel as of late 2025. If you want broader testing, ask Claude to build a universal binary (`pnpm tauri build --target universal-apple-darwin`). Adds ~30min to build time and ~2x to DMG size.
- **Gatekeeper friction is real.** Even with the install note on the landing page, a meaningful fraction of testers will bounce. The fix is a $99/yr Apple Developer Program → Developer ID Application cert → notarytool credentials. The release script already handles signing + notarization when those env vars are present; you'd just need to add `APPLE_SIGNING_IDENTITY` and `APPLE_NOTARYTOOL_PROFILE` to your shell and rerun `pnpm release:mac`.
- **No GitHub Actions release workflow.** Every release is currently a manual `pnpm release:mac` on this machine. When you have notarization set up, ask Claude to add `.github/workflows/release.yml` using `tauri-apps/tauri-action` so tag pushes auto-build and release.
- **No in-app updater.** Testers who installed beta-1 will not be auto-prompted for beta-2. For 1–5 testers this is fine; for >10 it's a pain. Tauri's updater plugin is the path.
- **No way to revoke a tester's access.** The beta gate is one shared invite code. If you want per-tester revocation, you'd want a small admin endpoint that issues per-user codes against Supabase.

---

## Status check — is the DMG done building?

Claude kicked off `pnpm release:mac` in the background. To check progress:
```sh
ls -la src-tauri/target/release/bundle/dmg/
# Look for Peer_0.2.0_arm64.dmg with today's timestamp.
```
If the timestamp is fresh (today), the build is done and Claude will rename it to `Peer.dmg` before publishing. If you see the May-10 file still, the build is still running or failed — paste the tail of the output to Claude.
