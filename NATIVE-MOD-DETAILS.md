# Seyrlaun Lite technical notes

Experimental world-space path tracing for Horizon. This repository contains
the complete native Rust integration and original WGSL, not a patched game.

Version: `0.1.0-alpha.3`

## Source

```text
horizon-native.json
mixins/offline_pt_lite/
  horizon-mixin.toml
  assets_*.rs
  game/*.rs
  renderer/*.rs
  shader_assets/
  data/
    path_tracer.wgsl
    path_tracer_ctm.wgsl
    LICENSE-Apache-2.0.txt
    LICENSE-Horizon.txt
    NOTICE.txt
    provenance.json
```

The Rust files handle scene/material transport, progressive accumulation,
render integration, configuration and controls. Both WGSL files are shader
source. They are not independently installable as a post-processing pack.

The `.hnmod` release asset packages this same source tree for Horizon's
native-mod build system. It is not an EXE or runtime DLL.

## Installation

Use Horizon's native-mod installation flow with a release `.hnmod`, or use
its **Add from GitHub** flow with the full alpha.3 release-page URL.
For pre-releases, a repository-home link can select another release or
fall back to branch source. Review the native-code trust warning,
explicitly build the selected native mods, then start the game through
the official Horizon launcher. The launcher still requires its normal
account and ownership verification.

Native mods run with the player's permissions and are not sandboxed.
No Minecraft resources, game executable, launcher, accounts or worlds are
included.

For automatic F2 denoising on Windows, download the
`windows-x64-with-oidn.zip` release asset. It includes the `.hnmod`, the
official OIDN runtime and its complete license/document directory. Copy
its `native-tools` directory into the Horizon game-data root, then install
and build the source mod through the original launcher. The launcher's
**Add from GitHub** action installs the mod, not this external runtime.
See `OIDN-RELEASE.md` for custom instances, checksums and detailed steps.
GitHub's automatic source ZIP does not contain OIDN binaries.

The native manifest pins the tested versions:

| Component | Revision |
| --- | --- |
| Horizon v0.2.97 | `a3acb5f0d2f59d2d5ff21a5aa000809ba1e35e2f` |
| horizon-mixins | `5a64af938ec86b00d53d16a408aabc0038fb3e57` |

Do not casually change the compatibility pins. A different engine revision
needs a fresh source application, compilation and in-game compatibility check.

## Controls and scope

F6 toggles path tracing by default; the key can be remapped. F8 opens
Lite's settings for rendering resolution, bounces, freeze and exposure.
F2 OIDN is enabled by default and can be disabled in the same settings.
In this compatibility alpha, avoid assigning PT keys that conflict with
custom vanilla key mappings: the older input API translates those keys
before the PT hook receives them.

With PT enabled, F2 snapshots scene-linear HDR radiance and generates
matching first-hit albedo and signed world-normal guides. A background
worker runs OIDN's RT filter with CPU/high-quality defaults, then blends
70% denoised radiance with 30% raw radiance before exposure and tone mapping.
The normal game view is not passed through OIDN.

Each capture writes four PNGs in the active instance's screenshots folder:
the main image, `-raw`, `-albedo` and `-normal`. The main image is denoised
when successful and remains raw if OIDN is unavailable or fails. Temporary
floating-point PFM inputs are private and cleaned up; the inspection PNG
guides are not the HDR denoiser inputs. Another F2 press while one capture
is processing reports that it is busy instead of silently replacing it.

This Lite still dispatches whole-frame samples. Capturing does not add a
sample-count threshold, but cannot read a half-finished GPU dispatch.

This preserved Lite renderer has a camera-relative 128-block scene,
CTM/PBR integration and its original accumulation/material behavior. It
does not include Pro, a weather controller or Full's extended camera/entity
features. Screenshot integration does not change the preserved transport.

Alpha limitation: F6 disables PT but does not remove compiled
sprite/CTM/mesher hooks or already loaded materials. For complete
removal, rebuild the instance without this mod. This version supplies
its own settings panel instead of relying on newer native-mod UI hooks
that do not exist in v0.2.97.

## Alpha.2 build failure

Alpha.2 was verified against a different, newer Horizon source revision
(`9d979ca`), not the official v0.2.97 source used by the launcher.
It requires `horizon-client-render` and native UI/screenshot APIs that
are absent from v0.2.97. The result is an `E_CRATE` failure before Rust
compilation. This is a package compatibility bug, not an OIDN, Rust
installation, account or instance-data problem.

Alpha.3 ports the integration to v0.2.97's monolithic `horizon-client`,
including its original draw/presentation and screenshot interfaces.
Do not fix alpha.2 by merely changing its revision pin or crate name.
Replace the package, then rebuild the same instance's native mods
through the original launcher.

## Building and packaging

The complete source required by the native mod is in this tree; the
private migration generator and its historical Lite worktree are not
required. Horizon's launcher applies the native source and compiles the
pinned engine.

An `.hnmod` must contain `horizon-native.json` at the archive root and
the native files under `mixins/offline_pt_lite/`. Root README/license
mirrors are repository documentation, not extra archive entries.
Do not ZIP the entire repository and merely rename it `.hnmod`.

Release artifacts should include the `.hnmod`, optional Windows OIDN ZIP
and `SHA256SUMS` from the same reviewed commit/tag. Keep OIDN binaries out
of source Git history. GitHub's source ZIP is an additional convenience.

## License

Original Lite additions are licensed under Apache-2.0; see `LICENSE`.
Existing/adapted Horizon portions retain MIT; see `LICENSE-Horizon.txt`
and `NOTICE`. Copies of these notices also travel inside the native bundle.
The optional OIDN runtime has its own upstream Apache-2.0 and third-party
notices, all retained with its binaries.

This project is not made or endorsed by Horizon or Mojang. Pro and its
weather controller are not part of this source tree or its license grant.
