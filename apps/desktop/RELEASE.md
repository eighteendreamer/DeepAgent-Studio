# DeepAgent Studio desktop release

## Version

The desktop installer version is controlled by:

- `apps/desktop/package.json`
- `apps/desktop/src-tauri/Cargo.toml`
- `apps/desktop/src-tauri/tauri.conf.json`

This release should set all three to the release version, for example `0.0.4`.

## Updater signing

Tauri updater packages must be signed. The client stores only the public key in
`src-tauri/tauri.conf.json`; keep the private key out of git.

Set these GitHub Actions secrets before publishing:

- `TAURI_SIGNING_PRIVATE_KEY`: contents of the private updater key
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: optional; leave empty for the current no-password key

The locally generated private key is in `apps/desktop/.tmp/deepagent-studio-updater.key`.
Copy its contents into the secret, then keep or delete the local file.

## Publishing installers

Run the GitHub Actions workflow `Release Desktop Installers`, or push a version
tag such as:

```bash
git tag v0.0.4
git push origin v0.0.4
```

The workflow builds installers on native runners:

- Windows: NSIS/MSI
- macOS: `.dmg`/`.app` bundles for Intel and Apple Silicon
- Linux: native Tauri Linux bundles

It uploads release assets and updater metadata to a draft GitHub Release.

## Update endpoints and mirrors

The app checks updater metadata from:

- GitHub Release: `https://github.com/eighteendreamer/DeepAgent-Studio/releases/latest/download/latest.json`
- Mirror: `https://download.deepagent.studio/releases/latest.json`

If users cannot connect to GitHub, mirror the release assets and `latest.json`
to the mirror host. Keep the asset URLs inside mirrored `latest.json` reachable
from the target region.

For environments with an HTTP proxy, set either:

- build-time `VITE_DEEPAGENT_UPDATE_PROXY`
- runtime `localStorage["deepagent.updateProxy"]`

The title-bar update button downloads an update now and installs it silently
when the app closes.

## On-demand speech runtimes

Speech models and the `whisper-cli` sidecar are managed runtimes. They must not
be bundled into the desktop installer; users download them on demand from the
runtime manager.

Linux uses the official whisper.cpp `v1.9.1` `whisper-bin-ubuntu-x64.tar.gz`
and `whisper-bin-ubuntu-arm64.tar.gz` assets with pinned SHA-256 hashes.

macOS needs DeepAgent-hosted CLI sidecar archives because upstream whisper.cpp
only publishes an `xcframework` for macOS, not a command-line `whisper-cli`
runtime. Publish these assets to the `runtime-whisper-cli-v1.9.1` GitHub
Release before building macOS installers:

- `deepagent-whisper-cli-macos-x64.tar.gz`
- `deepagent-whisper-cli-macos-arm64.tar.gz`

Then build the macOS desktop app with:

- `DEEPAGENT_WHISPER_CLI_MACOS_X64_SHA256`
- `DEEPAGENT_WHISPER_CLI_MACOS_ARM64_SHA256`

If the hash for the current macOS architecture is missing, the runtime remains
visible but installation is blocked fail-closed.

On Windows, auto-update should target the NSIS `.exe` artifact. The workflow is
already configured with `uploadUpdaterJson: true` and
`updaterJsonPreferNsis: true`, so MSI remains available for manual install
while `latest.json` points the updater at the NSIS package.

## Skill Marketplace

The desktop app ships a Skill Marketplace that loads skills from four roots —
`BuiltIn` (bundled `resources/skills/`), `User` (`~/.deepagent/skills/`),
`Installed` (`~/.deepagent/skills/marketplace/`), `Workspace`
(`<project>/.deepagent/skills/`) — and lets the user discover and install more
from [skillsmp.com](https://skillsmp.com). The 7 bundled skills are normalized
and copied during `npm run prebundle-skills` (run automatically by `pnpm build`)
into `apps/desktop/src-tauri/resources/skills/`.

### Manual smoke test (run before each release)

Project lacks a frontend e2e harness, so this is a human checklist. Open the
shipped installer (or `pnpm tauri dev`) and step through:

- [ ] Open the Skills view. The `Installed` tab lists at least the 7 bundled
      skills, each marked as `built_in`. Built-in skills do not show an
      uninstall button.
- [ ] Switch to the `Market` tab. Skill cards load (default sort by stars) and
      the result count is non-zero.
- [ ] Search for `browser` in the market search box. Cards refresh and the
      `agent-browser` family of skills shows up in the grid.
- [ ] Click the `+` button on any market skill. The `SkillInstallDialog` opens
      and shows the static scan report (file list + risk badges).
- [ ] AI Security Review streams text into the dialog while the install button
      stays disabled. After the verdict line (`PASS` / `FAIL`) the install
      button becomes clickable, with color and label matching the highest risk
      severity (green `Install Safe` / yellow `Install` / red `Install Anyway`).
- [ ] Click `Cancel`. The dialog closes, no skill is added, and no leftover
      directory remains under `~/.deepagent/skills/marketplace/`. After ~30
      minutes the in-memory `skills_pending` entry expires on its own.
- [ ] Reopen the dialog for the same skill, click the install button. A toast
      confirms install, the `Installed` tab now shows the skill with origin
      `installed`, and the `Uninstall` button works (skill disappears and the
      `~/.deepagent/skills/marketplace/<id>/` directory is removed).
- [ ] Open the Provider Config popover (gear icon). Type a clearly invalid API
      key, click `Test Connection`, and confirm the toast reports failure with
      the underlying error.
- [ ] Click `Clear` to remove the user key. The badge flips to `Builtin Key`
      (500/day), and a fresh search still returns results, proving the
      fallback to the built-in key works.
- [ ] Compose a message starting with `/`. The slash menu shows up to 8 skill
      candidates from `listSkills()`. Selecting one rewrites the input to
      `Please use the {name} skill: {rest}` and keeps focus in the composer.
- [ ] Open Settings → Skills. Confirm the four toggles persist across an app
      restart: `skill_catalog_enabled`, `skill_catalog_char_budget`,
      `skill_install_ai_review_enabled`, `skill_install_ai_review_model`.
- [ ] In a fresh chat session, verify the model receives the
      `<available-skills>` reminder once on turn 0 and does not get the same
      ids re-sent on later turns. After installing a new skill, the next turn
      includes the new id as a delta.

### Quality gates re-run before tagging

Run these from the repo root before pushing a release tag:

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo test --workspace --offline
cd apps/desktop && pnpm build
cd apps/desktop/src-tauri && cargo check --offline
```
