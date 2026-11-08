# Offline PT Lite

An open-source path tracer for taking screenshots in Horizon.
Hold the camera still and let the image get cleaner as it renders.
Lite is built for still images, not real-time gameplay.

**Current version: 0.1.0-alpha.2.** This is an experimental release.

[Download alpha.2](../../releases/tag/v0.1.0-alpha.2) | [Installation guide](OIDN-RELEASE.md)

## Features

- PBR material maps and connected textures from compatible resource packs.
- Adjustable render resolution, light bounces, exposure and lighting freeze.
- Automatic F2 screenshot denoising with Intel Open Image Denoise (OIDN).

Runs as a Horizon native mod (`.hnmod`), not an Iris or OptiFine shader pack.

<img width="2880" height="1736" alt="diowlmfe - Copy" src="https://github.com/user-attachments/assets/87b12e81-5e7e-488f-bd37-e4700234f781" />

<details>
<summary>View more screenshots</summary>
<img width="2880" height="1755" alt="2026-11-05_21 11 39" src="https://github.com/user-attachments/assets/7f29ea54-726d-4e27-87da-30716dcd0a0f" />
<img width="2880" height="1755" alt="2026-11-05_20 36 53" src="https://github.com/user-attachments/assets/c69ce389-0d11-412d-b93c-b759923b6502" />
<img width="2880" height="1755" alt="2026-11-05_17 59 29" src="https://github.com/user-attachments/assets/6ccd5f79-0575-4404-8c3b-7cb4256a0f1e" />
<img width="2880" height="1800" alt="jdsofkemokwfew" src="https://github.com/user-attachments/assets/1ab49af8-add6-4b27-acef-8babd5cd41c1" />
<img width="2880" height="1800" alt="dwqiodjams" src="https://github.com/user-attachments/assets/c671c903-d4c2-417e-9b01-a7fa6778f8e4" />
<img width="2592" height="1580" alt="2026-11-05_19 35 42_90" src="https://github.com/user-attachments/assets/7dfc0592-d026-4338-8541-b35c45e1dda5" />
<img width="2880" height="1755" alt="2026-11-05_20 53 35" src="https://github.com/user-attachments/assets/cd3ed6ca-e157-478a-a9a8-78a55de52c07" />


</details>

<details>
<summary>Output when you click f2</summary>
Normal
<img width="2880" height="1755" alt="2026-11-05_17 59 29-normal" src="https://github.com/user-attachments/assets/01d287d1-756d-400b-9da7-e0632aee95bb" />
   
Albedo
<img width="2880" height="1755" alt="2026-11-05_17 59 29-albedo" src="https://github.com/user-attachments/assets/e00f141e-1c21-473a-b69e-619bfda92102" />

Raw
<img width="2880" height="1755" alt="2026-11-05_17 51 51-raw" src="https://github.com/user-attachments/assets/3db6ebde-4abf-4229-bba7-4efcc32f1a37" />

Denoised
<img width="2880" height="1755" alt="2026-11-05_17 59 29" src="https://github.com/user-attachments/assets/1ee611c5-6eb5-4961-8a8b-e3254b290b61" />

</details>


## Install

1. Open the release linked above and download the Windows ZIP ending in
   `windows-x64-with-oidn.zip`.
2. Extract it and place `native-tools` in your Horizon game-data folder.
   The default Windows location is `%APPDATA%\horizon`.
3. In the original Horizon launcher, use **Add from GitHub** with the full
   alpha.2 release-page URL. Enable Lite, review the native-code warning
   and explicitly build the selected native mods.
4. Start that build through the launcher, open a world and press **F6**.

The first build can take a while. You need a Minecraft account that owns
the game. Native mods run with your permissions, so only install code you trust.

**Add from GitHub does not install OIDN.** Use the Windows release ZIP
and copy `native-tools` as above. For custom folders or local installation,
see the [installation guide](OIDN-RELEASE.md).

## Use

- F6 toggles path tracing; you can remap the key.
- F2 saves and denoises a screenshot while PT is on.
- Mod settings control quality, exposure, lighting freeze and denoising.

PT is off with fresh settings; saved preferences are remembered.
F2 denoising defaults to high quality. Four PNGs go in `screenshots`:
the main image, `-raw`, `-albedo` and `-normal`.
If OIDN is missing or fails, the main image stays raw. The matching
`-oidn.log` records the result and denoising time.

## Alpha limitations

Lite uses a local 128-block scene and has limited entity support.
The first image can take time; screenshots wait for a completed sample.
This alpha requires the Horizon build pinned in its manifest.

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
