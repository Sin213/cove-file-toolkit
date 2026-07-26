#!/usr/bin/env bash
#
# Local Linux release build (AppImage + .deb).
#
# Produces exactly the artifact names the GitHub Actions Linux job produces, so
# locally built files are drop-in replacements for CI ones.
#
# This exists because `tauri build` alone cannot produce a working AppImage on
# this host: linuxdeploy-plugin-gtk unconditionally copies libgiognutls.so into
# the AppDir and points GIO_EXTRA_MODULES at it. That module pulls in the HOST
# libgnutls -> HOST libleancrypto, while the AppDir ships its own differently
# built libleancrypto, and the two collide in ld.so — the app segfaults inside
# dlopen during startup. Deleting the module restores the known-good layout
# (the shipped 1.2.7 AppImage has an empty usr/lib/gio/modules). Nothing in the
# app needs GIO TLS: the updater uses ureq with its own TLS stack.
#
# NO_STRIP=true is also required — linuxdeploy's embedded strip fails against
# this host's binutils.
#
# Usage:
#   scripts/build-release-linux.sh              build + repair + stage + checksums
#   scripts/build-release-linux.sh --no-smoke   skip the launch smoke test (headless)
#   scripts/build-release-linux.sh --upload     also upload to the vVERSION GitHub release
#
# Upload is opt-in and never implied: it publishes to a public release.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

SMOKE=1
UPLOAD=0
for arg in "$@"; do
  case "$arg" in
    --no-smoke) SMOKE=0 ;;
    --upload)   UPLOAD=1 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

VERSION="$(node -p "require('./package.json').version")"
BUNDLE_DIR="src-tauri/target/release/bundle/appimage"
APPDIR="$BUNDLE_DIR/Cove Toolkit.AppDir"
DEST_DIR="release"
APPIMAGE_DEST="$DEST_DIR/Cove-File-Toolkit-${VERSION}-x86_64.AppImage"
DEB_DEST="$DEST_DIR/Cove-File-Toolkit-${VERSION}-amd64.deb"

echo "==> Building Cove File Toolkit ${VERSION} (appimage + deb)"
NO_STRIP=true ./node_modules/.bin/tauri build --bundles appimage,deb

# --- The fix, baked in -------------------------------------------------------
GIO_MODULE="$APPDIR/usr/lib/gio/modules/libgiognutls.so"
if [ -f "$GIO_MODULE" ]; then
  echo "==> Removing bundled libgiognutls.so (segfaults in dlopen at startup)"
  rm -f "$GIO_MODULE"
  echo "==> Repacking AppImage"
  REPACKED="$BUNDLE_DIR/Cove_Toolkit-x86_64.AppImage"
  # Drop any repack left by an earlier run so a failed repack cannot leave a
  # stale AppImage sitting here to be staged as if it were current.
  rm -f "$REPACKED"
  ( cd "$BUNDLE_DIR" && NO_STRIP=true ARCH=x86_64 \
      ~/.cache/tauri/linuxdeploy-plugin-appimage.AppImage --appdir "Cove Toolkit.AppDir" )
else
  echo "==> No bundled libgiognutls.so found; using the AppImage as built"
  REPACKED="$BUNDLE_DIR/Cove Toolkit_${VERSION}_amd64.AppImage"
fi

[ -f "$REPACKED" ] || { echo "expected AppImage not found: $REPACKED" >&2; exit 1; }

# --- Smoke test: never publish an AppImage that cannot start ------------------
if [ "$SMOKE" = "1" ]; then
  echo "==> Smoke testing the AppImage (20s)"
  set +e
  timeout 20 "$REPACKED" >/dev/null 2>&1
  rc=$?
  set -e
  # 124 = still running when the timeout fired, which is what a healthy GUI app
  # does. Anything else means it exited early — 139 is the dlopen segfault.
  if [ "$rc" != "124" ]; then
    echo "SMOKE TEST FAILED: AppImage exited with $rc instead of running" >&2
    echo "check the app log: ~/.local/share/cove-file-toolkit/logs/cove-file-toolkit.log" >&2
    exit 1
  fi
  echo "    ok — still running after 20s"
fi

# --- Stage with the CI's exact names + per-asset checksums -------------------
echo "==> Staging to $DEST_DIR/"
mkdir -p "$DEST_DIR"
cp "$REPACKED" "$APPIMAGE_DEST"
chmod +x "$APPIMAGE_DEST"

# Pin the .deb to this version explicitly. The bundle directory accumulates every
# version ever built here, and a glob picks whichever sorts first — that shipped a
# 0.1.0 package under a 1.2.7 name once already.
DEB_SRC="src-tauri/target/release/bundle/deb/Cove Toolkit_${VERSION}_amd64.deb"
[ -f "$DEB_SRC" ] || { echo "expected .deb not found: $DEB_SRC" >&2; exit 1; }
cp "$DEB_SRC" "$DEB_DEST"

( cd "$DEST_DIR"
  for f in "$(basename "$APPIMAGE_DEST")" "$(basename "$DEB_DEST")"; do
    sha256sum "$f" > "$f.sha256"
  done
)

echo
echo "Artifacts:"
ls -lh "$APPIMAGE_DEST" "$APPIMAGE_DEST.sha256" "$DEB_DEST" "$DEB_DEST.sha256"

if [ "$UPLOAD" = "1" ]; then
  echo
  echo "==> Uploading to release v${VERSION}"
  gh release upload "v${VERSION}" \
    "$APPIMAGE_DEST" "$APPIMAGE_DEST.sha256" \
    "$DEB_DEST" "$DEB_DEST.sha256" --clobber
  echo "    uploaded 4 assets (2 artifacts + 2 sidecars)"
else
  echo
  echo "Not uploaded. To publish:"
  echo "  gh release upload v${VERSION} \\"
  echo "    \"$APPIMAGE_DEST\" \"$APPIMAGE_DEST.sha256\" \\"
  echo "    \"$DEB_DEST\" \"$DEB_DEST.sha256\" --clobber"
fi
