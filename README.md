# SeyrlaunPT Lite

A lightweight, open-source offline path tracer for screenshots in
[Horizon](https://playhorizon.dev/). Compose your shot, hold the camera still
and let the image get cleaner as it renders. Lite is built for still images,
not real-time gameplay.

**Experimental release: `0.1.0-alpha.3` | Horizon `v0.2.97` | Native mod `.hnmod`**

[Download alpha.3](../../releases/tag/v0.1.0-alpha.3) |
[Install](#install) |
[Installation guide](OIDN-RELEASE.md) |
[Technical notes](NATIVE-MOD-DETAILS.md)

## Screenshots

<img src="https://github.com/user-attachments/assets/87b12e81-5e7e-488f-bd37-e4700234f781" alt="SeyrlaunPT Lite path-traced screenshot" width="100%" />

<details>
<summary>View the screenshot gallery</summary>

<table>
  <tr>
    <td width="50%"><img src="https://github.com/user-attachments/assets/7f29ea54-726d-4e27-87da-30716dcd0a0f" alt="SeyrlaunPT Lite gallery screenshot 1" width="100%" /></td>
    <td width="50%"><img src="https://github.com/user-attachments/assets/c69ce389-0d11-412d-b93c-b759923b6502" alt="SeyrlaunPT Lite gallery screenshot 2" width="100%" /></td>
  </tr>
  <tr>
    <td><img src="https://github.com/user-attachments/assets/6ccd5f79-0575-4404-8c3b-7cb4256a0f1e" alt="SeyrlaunPT Lite gallery screenshot 3" width="100%" /></td>
    <td><img src="https://github.com/user-attachments/assets/1ab49af8-add6-4b27-acef-8babd5cd41c1" alt="SeyrlaunPT Lite gallery screenshot 4" width="100%" /></td>
  </tr>
  <tr>
    <td><img src="https://github.com/user-attachments/assets/c671c903-d4c2-417e-9b01-a7fa6778f8e4" alt="SeyrlaunPT Lite gallery screenshot 5" width="100%" /></td>
    <td><img src="https://github.com/user-attachments/assets/7dfc0592-d026-4338-8541-b35c45e1dda5" alt="SeyrlaunPT Lite gallery screenshot 6" width="100%" /></td>
  </tr>
  <tr>
    <td colspan="2"><img src="https://github.com/user-attachments/assets/cd3ed6ca-e157-478a-a9a8-78a55de52c07" alt="SeyrlaunPT Lite gallery screenshot 7" width="100%" /></td>
  </tr>
</table>

</details>

## Features

- PBR material maps and connected textures from compatible resource packs.
- Adjustable render resolution, light bounces, exposure and lighting freeze.
- Automatic F2 screenshot denoising with optional Intel Open Image Denoise (OIDN).
- Separate raw, albedo and normal PNG outputs alongside the main image.

Lite runs as a Horizon native mod (`.hnmod`). It is not an Iris, OptiFine
or `.hshader` shader pack.

## Install

### Native mod

1. Open the **Download alpha.3** release linked above.
2. In the original Horizon launcher, choose **Add from GitHub** and enter
   the full alpha.3 release-page URL.
3. Enable Lite, review the native-code warning and explicitly **Build**
   the selected native mods.
4. Start that build through the launcher, open a world and press **F6**.

The first build can take a while. You need a Minecraft account that owns
the game. Native mods run with your permissions, so only install code you trust.

### Optional: OIDN denoising (Windows x64)

**The `.hnmod` and Add from GitHub do not install OIDN.** Install the runtime
separately to enable automatic F2 denoising:

1. Close the game and download the release ZIP ending in
   `windows-x64-with-oidn.zip`.
2. Extract it and place its `native-tools` folder in your active instance's
   game-data folder, beside `options.txt` and `saves`. The default Windows
   game-data root is `%APPDATA%\horizon`; custom instances may use another folder.
3. Keep the complete OIDN `bin` and `doc` folders together. The executable
   should be at:

   ```text
   <game-data>/native-tools/offline_pt_lite/oidn/bin/oidnDenoise.exe
   ```

Restart the game, enable PT and press F2. Without OIDN, screenshots are still
saved without denoising. For custom paths, verification and troubleshooting,
see the [installation guide](OIDN-RELEASE.md).

## Controls

| Key | Action |
| --- | --- |
| **F6** | Toggle path tracing; the key can be remapped. |
| **F8** | Open Lite's quality, exposure, lighting freeze and denoising settings. |
| **F2** | Save a PT screenshot and run OIDN if installed and enabled. |

PT is off with fresh settings; saved preferences are remembered.
F2 denoising defaults to high quality.

## Screenshot outputs

Each F2 capture saves four PNGs in the active instance's `screenshots` folder:

| File | Contents |
| --- | --- |
| `<capture>.png` | Main image: denoised when OIDN succeeds, raw otherwise. |
| `<capture>-raw.png` | Render before denoising. |
| `<capture>-albedo.png` | Albedo guide preview. |
| `<capture>-normal.png` | Normal guide preview. |

The matching `<capture>-oidn.log` records the denoising result and time.
There is no duplicate `-denoised.png`.

<details>
<summary>View F2 output examples</summary>

These images illustrate the output types. The raw and denoised examples
are from separate captures, not a matched before-and-after comparison.

<table>
  <tr>
    <th width="50%">Raw render</th>
    <th width="50%">Main output with OIDN</th>
  </tr>
  <tr>
    <td><img src="https://github.com/user-attachments/assets/3db6ebde-4abf-4229-bba7-4efcc32f1a37" alt="Raw render example before denoising" width="100%" /></td>
    <td><img src="https://github.com/user-attachments/assets/1ee611c5-6eb5-4961-8a8b-e3254b290b61" alt="Denoised main output example from a separate capture" width="100%" /></td>
  </tr>
  <tr>
    <th>Albedo guide</th>
    <th>Normal guide</th>
  </tr>
  <tr>
    <td><img src="https://github.com/user-attachments/assets/e00f141e-1c21-473a-b69e-619bfda92102" alt="Albedo guide output example" width="100%" /></td>
    <td><img src="https://github.com/user-attachments/assets/01d287d1-756d-400b-9da7-e0632aee95bb" alt="Normal guide output example" width="100%" /></td>
  </tr>
</table>

</details>

## Compatibility and limitations

- Alpha.3 targets Horizon `v0.2.97`, commit `a3acb5f`, as pinned in
  [`horizon-native.json`](horizon-native.json). Other revisions are unverified.
- Lite uses a local 128-block scene and has limited entity support.
- The first image can take time. Screenshots wait for a completed full-frame
  sample; this alpha does not capture an unfinished sample below 1 SPP.
- Use F6 to return to normal rendering. To fully remove Lite's material
  hooks, rebuild the instance without Lite.

<details>
<summary>Upgrading from alpha.2</summary>

Replace the old `.hnmod` with alpha.3 and rebuild native mods in the launcher.
Keep only one installed package with ID `offline_pt_lite`.

Alpha.2 targets a different engine layout and fails on v0.2.97 with
`E_CRATE: horizon-client-render is not a workspace member`. Creating another
instance or reinstalling OIDN does not fix that package. Existing OIDN 2.5.1
files can stay installed.

See the [compatibility fix](COMPATIBILITY-FIX.md) for details.

</details>

## Pro (in development)

A separate Pro version is in development, with atmospheric skies,
volumetric clouds, a weather controller and expanded camera tools.
Pro is not included in this repository or Lite's license.
Features and release timing are not final.

## License

Original Lite additions use [Apache-2.0](LICENSE). Horizon code retains its
[MIT license](LICENSE-Horizon.txt); see [NOTICE](NOTICE) for attribution.
OIDN's own license and third-party notices are included with its runtime.

This is an unofficial project, not made or endorsed by Horizon or Mojang.
