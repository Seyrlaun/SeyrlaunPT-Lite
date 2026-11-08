# SeyrlaunPT Lite

An open-source path tracer for taking screenshots in Horizon.
Hold the camera still and let the image get cleaner as it renders.
Lite is built for still images, not real-time gameplay.

**Current version: 0.1.0-alpha.3.** Experimental, for Horizon v0.2.97.

[Download alpha.3](../../releases/tag/v0.1.0-alpha.3) | [Installation guide](OIDN-RELEASE.md)

## Features

- PBR material maps and connected textures from compatible resource packs.
- Adjustable render resolution, light bounces, exposure and lighting freeze.
- Automatic F2 screenshot denoising with Intel Open Image Denoise (OIDN).

Runs as a Horizon native mod (`.hnmod`), not an Iris or OptiFine shader pack.

## Install

1. Open the release linked above and download the Windows ZIP ending in
   `windows-x64-with-oidn.zip`.
2. Extract it and place `native-tools` in your Horizon game-data folder.
   The default Windows location is `%APPDATA%\horizon`.
3. In the original Horizon launcher, use **Add from GitHub** with the full
   alpha.3 release-page URL. Enable Lite, review the native-code warning
   and explicitly build the selected native mods.
4. Start that build through the launcher, open a world and press **F6**.

The first build can take a while. You need a Minecraft account that owns
the game. Native mods run with your permissions, so only install code you trust.

**Add from GitHub does not install OIDN.** Use the Windows release ZIP
and copy `native-tools` as above. For custom folders or local installation,
see the [installation guide](OIDN-RELEASE.md).

## Use

- F6 toggles path tracing; you can remap the key.
- F8 opens Lite's settings.
- F2 saves and denoises a screenshot while PT is on.
- Settings control quality, exposure, lighting freeze and denoising.

PT is off with fresh settings; saved preferences are remembered.
F2 denoising defaults to high quality. Four PNGs go in `screenshots`:
the main image, `-raw`, `-albedo` and `-normal`.
If OIDN is missing or fails, the main image stays raw. The matching
`-oidn.log` records the result and denoising time.

## Alpha limitations

Lite uses a local 128-block scene and has limited entity support.
The first image can take time; screenshots wait for a completed sample.
This alpha requires the Horizon build pinned in its manifest.
Alpha.3 targets `a3acb5f`, the official v0.2.97 game source.

**Upgrading from alpha.2:** replace the old `.hnmod` and rebuild native
mods in the launcher. The old package targets a different engine layout
and fails with `E_CRATE: horizon-client-render is not a workspace member`.
Creating another instance or reinstalling OIDN does not fix that package.

Use F6 to return to normal rendering. To fully remove the mod's material
hooks, rebuild the instance without Lite.
See the [technical notes](NATIVE-MOD-DETAILS.md) for compatibility and internals.

## License

Original Lite additions use [Apache-2.0](LICENSE). Horizon code retains its
[MIT license](LICENSE-Horizon.txt); see [NOTICE](NOTICE) for attribution.
OIDN's own license and third-party notices are included with its runtime.

This is an unofficial project, not made or endorsed by Horizon or Mojang.

## Pro (in development)

A separate Pro version is in development, with atmospheric skies,
volumetric clouds, a weather controller and expanded camera tools.

Pro is not included in this repository or Lite's license.
Features and release timing are not final.
