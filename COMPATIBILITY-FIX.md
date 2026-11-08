# Alpha.3 Compatibility Fix

## What failed

Seyrlaun Lite alpha.2 targeted Horizon revision `9d979ca`. Horizon game
v0.2.97, selected through the original launcher, uses native source
revision `a3acb5f`, whose renderer still lives inside `horizon-client`.

The old package requests a `horizon-client-render` crate that does not
exist in that source tree, producing:

```text
E_CRATE: dependent 'horizon-client-render' is not a workspace member package
```

This is a compatibility mistake in the mod package. It is not caused by
your account, graphics card, OIDN, world, or Rust installation. A fresh
instance reproduces it because it builds the same incompatible package.

## How to recover

1. Close the game and let any existing native build finish.
2. Back up the old mod, then install the alpha.3 `.hnmod` in place of
   alpha.2. Keep only one package with mod ID `offline_pt_lite`.
3. Keep the existing OIDN 2.5.1 runtime, worlds, account and preferences.
4. In the original launcher, select the intended instance and rebuild
   its enabled native mods. Accept the native-code warning yourself.
5. Launch the resulting build through that launcher. F6 toggles PT;
   F8 opens Lite's settings, and F2 captures with optional OIDN denoising.

Alpha.3 supports the exact v0.2.97 source revision recorded in its
manifest. Other Horizon versions still require separate verification.
Do not remove the revision pin or rename one crate in alpha.2: its
render, asset, UI and screenshot APIs also need adaptation.

## For the maintainer

Publish the matching alpha.3 source tree, `.hnmod`, Windows OIDN companion
ZIP and checksums together. The corrected source is not a patched game
executable. Keep the old release available for provenance, mark its
compatibility problem clearly, and direct affected downloaders to the
replacement. Do not publish private build logs or local backup folders.

Suggested notice:

> The alpha.2 build error was caused by my mod targeting a different
> Horizon source layout. Please replace it with alpha.3 and rebuild
> native mods in the original launcher. Your OIDN installation and
> worlds do not need to be replaced.

This document describes the compatibility change. Consult the release
notes for completed validation; compilation alone does not establish
in-game rendering correctness.
