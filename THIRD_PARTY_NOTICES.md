# Third-Party Notices

This file records third-party material redistributed as part of DeepAgent
Studio, together with the license each item is redistributed under.

Rust and Node dependencies are declared in `Cargo.toml` / `package.json` and
resolved by their package managers; their notices are not duplicated here.
This file covers material **copied into this repository** and shipped inside the
application bundle, where the redistribution obligation is ours.

## Bundled plugins

Plugins under `apps/desktop/src-tauri/resources/plugins/` ship inside the
application. Every one of them must be classified in
`apps/desktop/src-tauri/bundled-plugins.json` as either first-party or
third-party, and `apps/desktop/scripts/check-plugin-licenses.cjs` enforces that
each third-party entry keeps its license file in-tree and appears in the table
below. An unclassified plugin fails CI.

| Plugin | Upstream | Version | License | License file |
| --- | --- | --- | --- | --- |
| `superpowers` | https://github.com/obra/superpowers | 5.1.3 | MIT | `resources/plugins/superpowers/LICENSE` |
| `figma` | https://www.figma.com | 2.0.13 | LicenseRef-Figma-Developer-Terms | `resources/plugins/figma/LICENSE.txt` |
| `boltz-api-cli` | https://boltz.bio | 0.1.1 | MIT | `resources/plugins/boltz-api-cli/LICENSE` |
| `wedecode` | https://gitee.com/xiaoshangongzuoshi/wxapkg | 0.9.1 | GPL-3.0-or-later | `resources/plugins/wedecode/LICENSE` |

### Open compliance item: `wedecode` corresponding source

`wedecode` is redistributed under GPL-3.0-or-later and its plugin directory
contains `runtime.zip` (~29 MB) in addition to the manifest and command
definition. GPL-3.0 §6 requires that whoever conveys object code also convey
the machine-readable **Corresponding Source**, through one of the routes §6a–§6e
allows — most practically §6d, offering equivalent access to the source from the
same place the object code is offered.

The license text is now shipped alongside the plugin, which satisfies §4/§5.
The Corresponding Source obligation is **not yet resolved** and needs a decision:

1. **Verify what `runtime.zip` contains.** If it is unmodified upstream
   published output, §6d can be satisfied by pointing at the upstream release
   next to the bundled artifact. If it was rebuilt or modified here, the
   modifications must be published and marked per §5a.
2. **Choose a route.** Either ship/point at the Corresponding Source, or stop
   conveying the object code — for example by downloading the runtime on first
   use instead of bundling it, which removes the conveyance entirely.
3. **Check aggregation.** GPL-3.0 §5 treats separate, independent works stored
   on one medium as an "aggregate", which does not extend the GPL to the rest of
   the bundle. Whether the plugin qualifies as an aggregate here depends on how
   the application invokes it and warrants review rather than assumption.

Until this is settled, treat `wedecode` as a known open item rather than a
cleared dependency.

## Reference material

`借鉴/` holds upstream projects consulted during development. It is a reference
tree, not a source of redistributed code, and nothing under it ships in the
application bundle. In particular `借鉴/claude-code` is Anthropic's repository
under "All rights reserved" and Anthropic's Commercial Terms of Service: its
plugins grant no redistribution right and must not be copied into
`resources/plugins/`. They can be offered as a marketplace source that the user
installs from, which is how Claude Code distributes them itself.
