# DeepAgent Studio desktop release

## Version

The desktop installer version is controlled by:

- `apps/desktop/package.json`
- `apps/desktop/src-tauri/Cargo.toml`
- `apps/desktop/src-tauri/tauri.conf.json`

For the first installer release these are set to `0.0.1`.

## Updater signing

Tauri updater packages must be signed. The client stores only the public key in
`src-tauri/tauri.conf.json`; keep the private key out of git.

Set these GitHub Actions secrets before publishing:

- `TAURI_SIGNING_PRIVATE_KEY`: contents of the private updater key
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: optional; leave empty for the current no-password key

The locally generated private key is in `.tmp/deepagent-studio-updater.key`.
Copy its contents into the secret, then keep or delete the local file.

## Publishing installers

Run the GitHub Actions workflow `Release Desktop Installers`, or push a version
tag such as:

```bash
git tag v0.0.1
git push origin v0.0.1
```

The workflow builds installers on native runners:

- Windows: NSIS/MSI
- macOS: `.dmg`/`.app` bundles for Intel and Apple Silicon
- Linux: native Tauri Linux bundles

It uploads release assets and updater metadata to a draft GitHub Release.

## Update endpoints and mirrors

The app checks updater metadata from:

- GitHub Release: `https://github.com/deepagent-studio/deepagent-studio/releases/latest/download/latest.json`
- Mirror: `https://download.deepagent.studio/releases/latest.json`

If users cannot connect to GitHub, mirror the release assets and `latest.json`
to the mirror host. Keep the asset URLs inside mirrored `latest.json` reachable
from the target region.

For environments with an HTTP proxy, set either:

- build-time `VITE_DEEPAGENT_UPDATE_PROXY`
- runtime `localStorage["deepagent.updateProxy"]`

The title-bar update button downloads an update now and installs it silently
when the app closes.
