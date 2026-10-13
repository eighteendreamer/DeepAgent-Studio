# Third-party notices

This plugin packages the precompiled npm distribution `rea-agents@6.4.0` from https://github.com/morluto/rea under MIT. The build verifies the npm lockfile integrity and retains the upstream license, third-party notices, dependency resources, and Windows native artifact metadata inside `runtime.zip`.

The plugin wrapper and its original Skill instructions belong to DeepAgent Studio and use the project's Apache-2.0 license. The Skill is original integration guidance; it does not copy the reference checkout's implementation.

The build adds `LICENSE-REA`, `REA-THIRD_PARTY_NOTICES.md`, and `DEPENDENCY_NOTICES.json` from the installed production distribution. Dependency license files remain in their original package directories inside the runtime archive.
