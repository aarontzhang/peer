#!/usr/bin/env bash
set -euo pipefail

APP_PATH="src-tauri/target/release/bundle/macos/Peer.app"
APP_BUNDLE_DIR="$(dirname "${APP_PATH}")"
DMG_DIR="src-tauri/target/release/bundle/dmg"
VERSION="$(node -p "require('./package.json').version")"
ARCH="$(uname -m)"
case "${ARCH}" in
  arm64) TAURI_ARCH="aarch64" ;;
  x86_64) TAURI_ARCH="x86_64" ;;
  *)
    echo "Unsupported macOS updater architecture: ${ARCH}" >&2
    exit 1
    ;;
esac
DMG_VERSIONED_PATH="${DMG_DIR}/Peer_${VERSION}_${ARCH}.dmg"
DMG_LATEST_PATH="${DMG_DIR}/Peer.dmg"
DMG_STAGING_DIR="${DMG_DIR}/staging"
DMG_RW_PATH="${DMG_DIR}/Peer_${VERSION}_${ARCH}-rw.dmg"
UPDATER_ASSET_NAME="Peer_${VERSION}_${TAURI_ARCH}.app.tar.gz"
UPDATER_TAR_PATH="${APP_BUNDLE_DIR}/${UPDATER_ASSET_NAME}"
UPDATER_SIG_PATH="${UPDATER_TAR_PATH}.sig"
UPDATER_LATEST_JSON="${APP_BUNDLE_DIR}/latest.json"
DMG_MOUNT="/Volumes/Peer"
DMG_BACKGROUND="src-tauri/dmg/background.tiff"
IDENTITY="${APPLE_SIGNING_IDENTITY:-}"
TEAM_ID="${APPLE_TEAM_ID:-}"
NOTARY_PROFILE="${APPLE_NOTARYTOOL_PROFILE:-}"
ALLOW_UNSIGNED="${PEER_ALLOW_UNSIGNED_RELEASE:-0}"
UPDATER_PRIVATE_KEY_PATH="${TAURI_SIGNING_PRIVATE_KEY_PATH:-${HOME}/.tauri/peer-updater.key}"
UPDATER_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"

if [[ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]]; then
  if [[ ! -f "${UPDATER_PRIVATE_KEY_PATH}" ]]; then
    echo "TAURI_SIGNING_PRIVATE_KEY_PATH is required for updater artifacts." >&2
    echo "Generate one with: pnpm tauri signer generate --ci -w ~/.tauri/peer-updater.key" >&2
    exit 1
  fi
  export TAURI_SIGNING_PRIVATE_KEY="$(cat "${UPDATER_PRIVATE_KEY_PATH}")"
fi

export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${UPDATER_PRIVATE_KEY_PASSWORD}"

if [[ "${ALLOW_UNSIGNED}" != "1" ]]; then
  if [[ -z "${IDENTITY}" ]]; then
    echo "APPLE_SIGNING_IDENTITY is required for a public macOS release." >&2
    echo "Use a Developer ID Application identity, or set PEER_ALLOW_UNSIGNED_RELEASE=1 for a local smoke build." >&2
    exit 1
  fi

  if [[ "${IDENTITY}" != Developer\ ID\ Application:* ]]; then
    echo "APPLE_SIGNING_IDENTITY must be a Developer ID Application identity for public distribution." >&2
    echo "Current value: ${IDENTITY}" >&2
    exit 1
  fi

  if [[ -z "${NOTARY_PROFILE}" ]]; then
    echo "APPLE_NOTARYTOOL_PROFILE is required for a public macOS release." >&2
    echo "Create one with: xcrun notarytool store-credentials <profile-name>" >&2
    exit 1
  fi
elif [[ -z "${IDENTITY}" ]]; then
  echo "PEER_ALLOW_UNSIGNED_RELEASE=1: building a local ad-hoc signed DMG that Gatekeeper will reject." >&2
fi

strip_launchservices_blockers() {
  local plist="${APP_PATH}/Contents/Info.plist"
  if /usr/libexec/PlistBuddy -c "Print :LSRequiresCarbon" "${plist}" >/dev/null 2>&1; then
    /usr/libexec/PlistBuddy -c "Delete :LSRequiresCarbon" "${plist}"
  fi
}

pnpm sidecar
pnpm tauri build --bundles app

strip_launchservices_blockers

if [[ -n "${IDENTITY}" ]]; then
  codesign --force --deep --options runtime \
    --timestamp \
    --sign "${IDENTITY}" \
    --entitlements src-tauri/entitlements.plist \
    "${APP_PATH}"
else
  codesign --force --deep --sign - "${APP_PATH}"
fi

codesign --verify --deep --strict --verbose=2 "${APP_PATH}"

rm -f "${UPDATER_TAR_PATH}" "${UPDATER_SIG_PATH}" "${UPDATER_LATEST_JSON}"
COPYFILE_DISABLE=1 tar -czf "${UPDATER_TAR_PATH}" -C "${APP_BUNDLE_DIR}" "Peer.app"
if [[ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]]; then
  pnpm tauri signer sign -p "${UPDATER_PRIVATE_KEY_PASSWORD}" "${UPDATER_TAR_PATH}"
else
  pnpm tauri signer sign -f "${TAURI_SIGNING_PRIVATE_KEY_PATH}" -p "${UPDATER_PRIVATE_KEY_PASSWORD}" "${UPDATER_TAR_PATH}"
fi

UPDATE_SIGNATURE="$(cat "${UPDATER_SIG_PATH}")"
UPDATE_PUB_DATE="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
UPDATE_URL="https://github.com/aarontzhang/peer/releases/latest/download/${UPDATER_ASSET_NAME}"
UPDATE_PLATFORM="darwin-${TAURI_ARCH}"
VERSION="${VERSION}" \
UPDATE_SIGNATURE="${UPDATE_SIGNATURE}" \
UPDATE_PUB_DATE="${UPDATE_PUB_DATE}" \
UPDATE_URL="${UPDATE_URL}" \
UPDATE_PLATFORM="${UPDATE_PLATFORM}" \
node --input-type=module > "${UPDATER_LATEST_JSON}" <<'NODE'
const version = process.env.VERSION;
const platform = process.env.UPDATE_PLATFORM;
const payload = {
  version,
  notes: `Peer ${version}`,
  pub_date: process.env.UPDATE_PUB_DATE,
  platforms: {
    [platform]: {
      signature: process.env.UPDATE_SIGNATURE,
      url: process.env.UPDATE_URL,
    },
  },
};
process.stdout.write(`${JSON.stringify(payload, null, 2)}\n`);
NODE

mkdir -p "${DMG_DIR}"
rm -rf "${DMG_STAGING_DIR}"
rm -f "${DMG_VERSIONED_PATH}" "${DMG_LATEST_PATH}" "${DMG_RW_PATH}"
mkdir -p "${DMG_STAGING_DIR}/.background"
cp -R "${APP_PATH}" "${DMG_STAGING_DIR}/Peer.app"
ln -s /Applications "${DMG_STAGING_DIR}/Applications"
cp "${DMG_BACKGROUND}" "${DMG_STAGING_DIR}/.background/background.tiff"

hdiutil create -volname "Peer" \
  -srcfolder "${DMG_STAGING_DIR}" \
  -ov \
  -format UDRW \
  "${DMG_RW_PATH}"

if hdiutil info | grep -q "${DMG_MOUNT}"; then
  hdiutil detach "${DMG_MOUNT}" >/dev/null
fi

hdiutil attach -readwrite -noverify -noautoopen "${DMG_RW_PATH}" >/dev/null

osascript <<'APPLESCRIPT'
tell application "Finder"
  tell disk "Peer"
    open
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    set bounds of container window to {100, 100, 760, 500}
    set theViewOptions to the icon view options of container window
    set arrangement of theViewOptions to not arranged
    set icon size of theViewOptions to 96
    set background picture of theViewOptions to file ".background:background.tiff"
    set position of item "Peer.app" of container window to {165, 205}
    set position of item "Applications" of container window to {495, 205}
    update without registering applications
    delay 1
    close
  end tell
end tell
APPLESCRIPT

SetFile -a V "${DMG_MOUNT}/.background" || true
sync
hdiutil detach "${DMG_MOUNT}" >/dev/null
hdiutil convert "${DMG_RW_PATH}" -format UDZO -imagekey zlib-level=9 -o "${DMG_VERSIONED_PATH}" >/dev/null
rm -f "${DMG_RW_PATH}"
rm -rf "${DMG_STAGING_DIR}"

if [[ -n "${IDENTITY}" ]]; then
  codesign --force --timestamp --sign "${IDENTITY}" "${DMG_VERSIONED_PATH}"
fi

if [[ -n "${IDENTITY}" && -n "${NOTARY_PROFILE}" ]]; then
  notary_args=(--keychain-profile "${NOTARY_PROFILE}")
  if [[ -n "${TEAM_ID}" ]]; then
    notary_args+=(--team-id "${TEAM_ID}")
  fi
  xcrun notarytool submit "${DMG_VERSIONED_PATH}" \
    "${notary_args[@]}" \
    --wait
  xcrun stapler staple "${DMG_VERSIONED_PATH}"
fi

cp "${DMG_VERSIONED_PATH}" "${DMG_LATEST_PATH}"

echo "Built DMG at ${DMG_VERSIONED_PATH}"
echo "Website upload asset at ${DMG_LATEST_PATH}"
echo "Built updater bundle at ${UPDATER_TAR_PATH}"
echo "Built updater signature at ${UPDATER_SIG_PATH}"
echo "Upload updater manifest at ${UPDATER_LATEST_JSON}"
