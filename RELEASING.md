# Releasing Pady

1. Update `version` in the root `Cargo.toml`. The app, the installers and the CLI all read it from there.
2. Move the **Unreleased** notes in `CHANGELOG.md` under the new version.
3. Commit, then tag and push:
   ```bash
   git tag v0.2.0
   git push origin v0.2.0
   ```
4. The **Release** workflow builds the installers on Windows and Linux and attaches them to a **draft** release, together with the CLI archives and `SHA256SUMS-*.txt`. Check the files, edit the notes, then publish.

To try a build without releasing, run **Actions › Release › Run workflow**. The installers appear as workflow artifacts.

## What gets built

| File | Platform | Notes |
|---|---|---|
| `Pady_<version>_x64-setup.exe` | Windows 10/11 | Per-user NSIS installer, no admin rights needed. Installs WebView2 if it's missing; the VC++ runtime is linked statically. |
| `Pady_<version>_x64_en-US.msi` | Windows | Per-machine MSI for managed deployment (Intune, Group Policy). |
| `Pady_<version>_amd64.deb` | Debian, Ubuntu, Mint | Declares its libraries and installs a udev rule, so touchpad access works without joining the `input` group. |
| `Pady-<version>-1.x86_64.rpm` | Fedora, openSUSE | Same as the .deb. |
| `Pady_<version>_amd64.AppImage` | Any x86-64 Linux | Portable. On first launch, **Fix permissions** installs the udev rule. |
| `pady-cli-<version>-*.zip / .tar.gz` | Both | The optional command-line tool. |

## Code signing (Windows)

Unsigned installers work, but SmartScreen shows "Windows protected your PC" until a download builds reputation. To sign, add two repository secrets:

- `WINDOWS_CERTIFICATE`: your code-signing certificate (`.pfx`), base64-encoded, for example `[Convert]::ToBase64String([IO.File]::ReadAllBytes("cert.pfx"))`.
- `WINDOWS_CERTIFICATE_PASSWORD`: its password.

The workflow notices them and signs the app, the setup and the MSI. Nothing else needs to change.
