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

## Auto-update

Installed copies of Pady check `releases/latest/download/latest.json` twice a day and offer one-click updates. Update files are signed with Pady's updater key; the app only installs files signed with it (the public key is in `crates/pady-app/tauri.conf.json`).

One-time setup, in **Settings › Secrets and variables › Actions › New repository secret**:

- `TAURI_SIGNING_PRIVATE_KEY`: the contents of the private key file (`~/.tauri/pady-updater.key`).
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: its password (the current key has none, so leave the value empty or skip this secret).

With the key in place, every tagged release also gets `.sig` files and a `latest.json`; publishing the release is what makes it reach everyone. **Keep a backup of the private key**: without it, installed copies can never be updated again (a new key needs a new public key in the app, so people would have to reinstall once).

Windows installs (setup and MSI) and the AppImage update themselves. The .deb and .rpm are told about new versions and update through the package manager or a new download.

## Code signing with SignPath (free for open source)

[SignPath Foundation](https://signpath.org) signs open-source projects for free, which removes the "Windows protected your PC" warning.

1. Apply at [signpath.org/apply](https://signpath.org/apply) with the repository link (Pady is MIT licensed, which they require). Approval takes a little while.
2. Once approved, in SignPath create the project `Pady`, a signing policy `release-signing` and an artifact configuration `installers` that signs the `.exe` and `.msi` files, and link the GitHub repository as a trusted build system.
3. In the repository, add the secret `SIGNPATH_API_TOKEN` and the variable `SIGNPATH_ORGANIZATION_ID` (**Settings › Secrets and variables › Actions**). Use the variables `SIGNPATH_PROJECT_SLUG`, `SIGNPATH_SIGNING_POLICY_SLUG` and `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG` only if you named things differently.

The Release workflow then sends the setup and MSI to SignPath, waits for the signed files, re-signs them for the updater and publishes those. Nothing else changes.

## Code signing with your own certificate (Windows)

If you buy a certificate instead, add two repository secrets:

- `WINDOWS_CERTIFICATE`: your code-signing certificate (`.pfx`), base64-encoded, for example `[Convert]::ToBase64String([IO.File]::ReadAllBytes("cert.pfx"))`.
- `WINDOWS_CERTIFICATE_PASSWORD`: its password.

The workflow notices them and signs the app, the setup and the MSI. Nothing else needs to change.
