# Offline PT Lite: Windows OIDN Runtime

This optional Windows x64 package contains the Lite native source mod and
unmodified Intel Open Image Denoise (OIDN) 2.5.1 runtime. It contains no game
executable, launcher, Minecraft resources, account data, worlds or external packs.
The game-side screenshot worker is Rust; there is no player-facing script and
no automatic runtime download.

## Package Layout

```text
README.md
LICENSE
LICENSE-Horizon.txt
NOTICE
SHA256SUMS
release-manifest.json
horizon-native-mods/
  offline_pt_lite.hnmod
native-tools/offline_pt_lite/oidn/
  runtime-manifest.json
  bin/
    oidnDenoise.exe
    OpenImageDenoise*.dll
    sycl9.dll
    tbb*.dll
    ur*.dll
  doc/
    LICENSE.txt
    third-party-programs.txt
    third-party-programs-oneTBB.txt
    third-party-programs-DPCPP.txt
    README.md
    CHANGELOG.md
    readme.pdf
```

The runtime is deliberately outside `.hnmod`: source mods have an unpacked
size limit and the launcher does not install arbitrary staged data beside
the running game. Keep the entire `bin` and `doc` directories together.
The CLI and all 15 upstream DLLs are included, even though Lite defaults to
CPU denoising. SDK headers, import libraries, benchmarks and tests are omitted.

## Install Through Horizon

1. Install and prepare the supported Horizon version through the original
   Horizon launcher, using an account that owns the game. Keep its ordinary
   authentication and ownership checks.
2. Verify the release asset's SHA-256 against the release `SHA256SUMS`, then
   extract the outer ZIP into a temporary folder. It is not a game install.
3. Copy `native-tools` into the Horizon game-data root, beside `options.txt`
   and `saves`. On a default Windows install this is `%APPDATA%\horizon`.
   If the launcher uses a custom `HORIZON_HOME`, use that root instead.
   Replace the old mod-specific OIDN directory as a complete unit after
   closing the game; do not mix DLLs from different releases.
4. Use the original launcher's **Add from GitHub** flow when a matching
   release is published. Read and explicitly accept its native-code warning,
   then use **Settings > Native mods > Build**. For a local reviewed
   `.hnmod`, unpack that ZIP-format mod into its own folder containing
   `horizon-native.json` and `mixins`. The original launcher's Rust CLI
   supports `horizon-launcher native-build <unpacked-mod-folder>
   --trust-native-code`; it does not accept the packed `.hnmod` as this
   command's input. Merely copying a `.hnmod` does not enable or build it.
5. Start the resulting native-mod build through the original launcher.
   Building from the default Horizon instance creates/uses the **Native
   mods** instance. A source build of an existing custom instance keeps
   that instance's data location. Use F6 to enable PT and F2 to capture.

Adding a mod from GitHub installs its `.hnmod` source asset, not the external
OIDN files in this outer ZIP. The runtime copy in step 3 is still required.
Use the complete Windows ZIP for OIDN, not GitHub's automatic source archive.

For a local GUI-only install without a published GitHub release:

1. Copy the packaged `horizon-native-mods/offline_pt_lite.hnmod` into that
   same relative location below the chosen instance's game home.
2. Start that ordinary instance through the original launcher, open
   **Options > Mods**, and turn on the listed **Installed, not built**
   Lite mod. Exit the game. This records the explicit mod-enabled choice.
3. With that same instance selected in the launcher, turn on **Settings >
   Game > Native mods** if it is off, then select **Build** and explicitly
   accept the warning naming Lite.
4. Start the resulting native build. A build from the default Horizon home
   uses the separate **Native mods** instance; it does not move the
   default instance's worlds. Select/open the world you intend to use in
   the resulting instance.

The native builder discovers `.hnmod` files only in the selected instance's
`horizon-native-mods` folder and includes only mods explicitly turned on.
The outer ZIP is not itself an installer; extraction alone does not modify
launcher settings, accept trust warnings, enable mods or start a build.

The first native build can download the launcher's Rust toolchain and take
substantially longer than later builds. This package does not bypass that
build or alter the launcher. A fresh installation test on a second machine
is still required before calling the alpha release verified.

## Runtime Lookup

The packaged executable's relative path is:

```text
native-tools/offline_pt_lite/oidn/bin/oidnDenoise.exe
```

Set `HORIZON_OIDN_BIN`, if needed, to the absolute path of the official
`oidnDenoise.exe`. The screenshot worker checks a nonempty override first;
an invalid explicit path/name reports an error instead of silently
running a different binary.
It next checks that relative path below nonempty `HORIZON_HOME`, or below
the working directory when `HORIZON_HOME` is unset. Official launcher
launches set the working directory to the game home; for owned instances
they also set `HORIZON_HOME` to the same location. If manually adding the
game's `--home` argument, align `HORIZON_HOME` with it or set the explicit
OIDN executable override.

The launcher may run the compiled native game from
`<root>/instances/native-mods` rather than `<root>`. To share one runtime
across official instances, the worker additionally recognizes only
these exact layouts and tries the same runtime suffix below `<root>`:

```text
<root>/instances/<id>
<root>/ports/<id>/home
<root>/horizon-packs/<id>/home
```

There is no unrestricted ancestor search or scan of Windows profiles.
For an arbitrary relocated instance, place `native-tools` in that
instance's own home or configure the explicit absolute override. The last
compatibility lookups are `oidn/bin/oidnDenoise.exe` beside the game
executable, then an official `oidnDenoise.exe` on PATH. The launcher version
folder is not the supported installation location for this package.

OIDN is optional: a missing or failed runtime preserves the raw screenshot
and reports the denoising failure. It never downloads or installs another
runtime silently.

## Screenshot Scope

Lite alpha.2's F2 path captures accumulated radiance and matching
albedo/normal guides from the next completed full-frame film, then runs
OIDN asynchronously with CPU device and high quality defaults. It adds no
artificial sample-count threshold, but Lite schedules complete full-frame
samples: F2 cannot read a partially finished GPU dispatch or promise a
capture below 1 spp. PNG deliverables are written to the active instance's
`screenshots` folder; floating-point intermediates are private temporary
files, not substitutes for the public PNGs.

The outputs share a timestamped base name:

```text
<capture>.png
<capture>-raw.png
<capture>-albedo.png
<capture>-normal.png
<capture>-oidn.log
```

The main PNG initially contains the raw image, then is atomically replaced
with a 70% denoised / 30% raw blend in linear space. There is no duplicate
`-denoised.png`. The albedo and normal PNGs are inspection images; OIDN
receives the original floating-point guides in private temporary PFMs,
not these display-encoded PNGs. `clean_aux` is not enabled.

The CPU worker defaults to half the logical CPU threads (at least one),
with affinity disabled. On Windows its child process is hidden and runs
below normal priority. `HORIZON_OIDN_DEVICE` explicitly overrides the
default CPU device. If OIDN fails or exceeds 180 seconds, the worker
retains all four PNGs, leaves the main image raw and records an error log.
Termination/reaping is attempted only for the OIDN child it created; OS
termination errors are reported instead of waiting indefinitely. Temporary
PFMs are cleaned.

## Pinned Upstream Files

- Product/version: Intel Open Image Denoise `2.5.1`, Windows x64.
- Official repository: <https://github.com/RenderKit/oidn>.
- Release: <https://github.com/RenderKit/oidn/releases/tag/v2.5.1>.
- Archive: <https://github.com/RenderKit/oidn/releases/download/v2.5.1/oidn-2.5.1.x64.windows.zip>.
- Archive size: `56619412` bytes.
- Archive SHA-256:
  `F11F91BC072A5E3A564515724CB72AB8FCFBC445C84A84C197FF9B16CC01396F`.

`runtime-manifest.json` records all 23 shipped upstream files, their sizes
and SHA-256 digests. The packager checks the official archive digest,
validates each selected archive entry, copies it without modification and
checks the copied file again. It rejects any unlisted upstream DLL or file
in `doc`, ensuring the complete upstream license/document directory travels
with the runtime.

OIDN's own license is Apache-2.0. Its dependencies have separate terms:
retain all upstream license and third-party-programs files, not merely
OIDN's main license. Their inclusion is not a complete legal audit or an
endorsement by Intel, Horizon or Mojang.

## Maintainer Packaging

`tools/Package-PtNativeRelease.ps1` is a PowerShell 7 developer tool, not an
installer. It accepts a reviewed source-only `.hnmod` and the already
downloaded pinned official OIDN archive. It never fetches anything.

The public GitHub source tree includes this tool and its metadata:

```text
tools/Package-PtNativeRelease.ps1
LICENSE                                  # Original additions: Apache-2.0
LICENSE-Horizon.txt                       # Existing/adapted Horizon: MIT
NOTICE
OIDN-RELEASE.md
oidn-2.5.1-windows-x64.manifest.json
```

Run the tool from that source tree without a private generator, historical
checkout or external metadata directory. For the development-tree layout,
it also supports metadata under `tools/pt_native_bundle`: the same OIDN
document, manifest and `NOTICE`, with Apache text in `LICENSE-APACHE-2.0`
and Horizon's MIT text in the repository-root `LICENSE`. If the development
manifest is present it selects that entire layout; otherwise it selects
the public root layout. It does not mix missing files between layouts.

GitHub's automatic source ZIP contains the Rust/WGSL source, documents,
manifest and packaging tool. It contains neither the official OIDN
runtime binaries nor a prepackaged `.hnmod`. Download the reviewed release
assets for installation; the Windows release ZIP contains OIDN's binaries
and notices. The packaging tool below consumes a separately reviewed
`.hnmod`; it does not create that mod from source. A native-mod archive
contains only `horizon-native.json` and `mixins/offline_pt_lite/`, not the
repository's root documents or `tools` directory. Do not rename the
automatic GitHub source ZIP to `.hnmod`.

```powershell
.\tools\Package-PtNativeRelease.ps1 `
  -NativeMod <reviewed-alpha.2.hnmod> `
  -ExpectedNativeModSha256 <reviewed-64-character-SHA256> `
  -OidnArchive <oidn-2.5.1.x64.windows.zip> `
  -OutputDirectory <new-release-output-directory>
```

Add `-VerifyOnly` to validate all inputs without creating output. The
output parent must already exist; the destination itself must not exist.
The tool never merges, overwrites, installs, uploads, launches or recursively
deletes anything. An interrupted/failed output is left for inspection and
cannot be reused accidentally.

The output contains a standalone `.hnmod`, the Windows ZIP, `SHA256SUMS`
for those two assets, and an inspectable `payload` directory. The ZIP's
own checksums and release manifest cover its payload. Publish only reviewed
release assets from the same commit/tag, not private workspace folders,
build logs, patched game executables or backup directories.

A maintainer test using a preserved alpha.1 `.hnmod` verifies packaging
only. Alpha.1 has no automatic OIDN screenshot integration; bundling its
source with an OIDN runtime does not add that feature.
