This file covers release behavior for this repository.

Do not cut a release unless I explicitly ask for a build, package, or release task.

If I ask for a Windows build, default to a local portable .exe unless I explicitly ask for an installer.

Before asking where release output should go, search for the previous successful build directory and reuse it if possible.

When publishing binaries, include a matching .sha256 sidecar for each shipped artifact using standard sha256sum output format.

For GitHub repos under ~/Projects/, every shipped binary should have a corresponding .sha256 file alongside it on release.

Prefer the existing naming pattern, output directory, and packaging method used by prior successful releases.

When the release task is complete, report the exact artifact names, exact output paths, and whether checksum files were generated.

If external dependencies are still required, state that clearly at the end of the release task.
## Packaging Discipline

- Do not introduce new packaging methods unless the repo already defines them.
- Do not restructure build outputs during a release task.
- Follow the exact packaging flow used in prior successful builds.

## Release Checklist

### Pre-release

- [ ] Version bump in `src-tauri/tauri.conf.json` and `src-tauri/Cargo.toml`
- [ ] `npm run tauri build` completes without errors
- [ ] Smoke-test the AppImage or .deb on a clean system

### Linux artifacts (built on Linux host)

```
src-tauri/target/release/bundle/appimage/cove-file-toolkit_<ver>_amd64.AppImage
src-tauri/target/release/bundle/deb/cove-file-toolkit_<ver>_amd64.deb
```

### Windows artifacts (built on Windows host)

```
src-tauri/target/release/bundle/nsis/Cove File Toolkit_<ver>_x64-setup.exe
```

> Portable .exe is not a defined packaging target. Do not ship `target/release/cove-file-toolkit.exe` as a release artifact without an explicit packaging step.

### Generate checksums

```bash
cd src-tauri/target/release/bundle
sha256sum appimage/cove-file-toolkit_*_amd64.AppImage > appimage/cove-file-toolkit_*_amd64.AppImage.sha256
sha256sum deb/cove-file-toolkit_*_amd64.deb > deb/cove-file-toolkit_*_amd64.deb.sha256
# On Windows:
sha256sum nsis/Cove\ File\ Toolkit_*_x64-setup.exe > nsis/Cove\ File\ Toolkit_*_x64-setup.exe.sha256
```

### Post-release

- [ ] Upload artifacts + `.sha256` files to GitHub release
- [ ] Verify checksums: `sha256sum -c <file>.sha256`
- [ ] Tag the release commit: `git tag v<ver>`
