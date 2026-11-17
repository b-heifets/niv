# niv

Fast keyboard-driven NIfTI slice viewer for Kitty-compatible terminals, including Ghostty and Rex. It is a standalone Rust executable. It reads each NIfTI volume once into RAM, displays grayscale and colored intensity layers, and draws an optional Allen CCFv3 ID-atlas contour as the top layer.

**v0.6 — experimental.** The source has been checked, but cannot be compiled or tested with Kitty/Rex in this environment. Please run `cargo test` and try it locally before committing.

## Build / update

```bash
cd ~/Documents/_GitHub/niv
cargo test
cargo install --path . --force
niv --version
```

To test without a terminal graphics client:

```bash
niv data/sample.nii.gz --bench 50
```

## Try the included aligned atlas demo

```bash
niv data/sample.nii.gz --axis z \
  --overlay data/demo_activation.nii.gz --overlay-window 0.5,4 \
  --atlas data/demo_atlas.nii.gz --lut data/demo_lut.csv
```

The bundled `data/sample.nii.gz`, `data/demo_atlas.nii.gz` and `data/demo_activation.nii.gz` share a grid. The CCF regional summary itself is **not bundled**; use the local copy from UNRAVEL or download it separately.

## Compact interface

Normal mode reserves one line above and three lines below the viewer. No image dimensions, axis, rotation, title or quit hint are shown. The title line shows a bold **Region:** and the region abbreviation/name in the atlas LUT color when inspecting a voxel; otherwise it shows the atlas file name. Atlas and other NIfTI basenames have the `.nii.gz` suffix removed. The atlas title shares the region line when space allows.

The first bottom line shows the **selected layer** (name, palette, display min/max and actual data range). The second line shows raw intensities for **all loaded layers** at the hovered/keyboard-inspected voxel (compact index:value pairs). The last line is a dim keyboard hint. Region colors come from the loaded CSV LUT.

`Tab` switches to **image-only** mode: all text is hidden, and the viewer scales to the maximum available terminal area while preserving physical aspect ratio. `Tab` again restores the compact interface. `t` optionally opens a compact layer list, shrinking the viewer only while it is open. Press `1` for the base image, `2` for the first overlay, and so forth, up to `9`.

### Slice and viewer controls

| Keys | Action |
| --- | --- |
| Arrow keys or `h`/`j`/`k`/`l` | Move slices (or move crosshair when inspecting) |
| Shift + direction | ×2 |
| Ctrl + direction | ×3 |
| Alt/Option + direction | ×4 |
| `[` / `]` | Decrease/increase base stride by exactly 1 (range 1–256) |
| Mouse wheel / two-finger trackpad scroll | Change slices using current stride and modifier multipliers |
| PgUp / PgDn | Change slices by 25, even when inspecting |
| Home / End | First/last slice |
| `x` / `y` / `z` | Set slicing axis (array index axis) |
| `r` / `R` | Rotate view 90° clockwise/counterclockwise |
| `f` / `v` | Flip horizontally/vertically |
| `a` | Show/hide atlas |
| `o` | Toggle all intensity overlays |
| `i` | Keyboard inspector on/off (arrows then move crosshair) |
| `Tab` | Toggle image-only mode |
| `t` | Show/hide compact list of layers |
| `q`, `Esc`, Ctrl+C | Exit (when not editing) |

Mouse gestures work when the terminal and any Rex/SSH intermediates forward scroll and mouse events. Touch on iOS Rex may not generate mouse-hover events; keyboard inspection remains available. To request pixel-precision mouse coordinates on terminals supporting SGR 1016, pass `--mouse-pixels` (test in local Ghostty first). Mouse movement updates **only the inspector text**, not the large Kitty image.

### Selected-layer controls

| Keys | Action |
| --- | --- |
| `1`–`9` | Select base (1) or overlay (2 and above) |
| `e` | Type `min,max` and press Enter (Esc cancels) |
| `m` / `M` | Type minimum or maximum individually |
| `c` | Cycle grayscale → warm → cool → saturated → grayscale |
| `+` / `-` | Narrow / widen selected layer's display window |
| `,` / `.` | Shift selected layer's window down/up |
| `0` | Reset selected layer's original window |
| Space | Show/hide the selected overlay (not the base) |

**Color scales:** gray uses black→white; warm uses red→yellow; cool uses blue→cyan. Saturated assigns each layer a stable, distinct vivid hue, rendered from dark→bright. CLI `--overlay-color HEX` starts that layer in its legacy fixed-tint mode; press `c` to switch to grayscale and cycle onward. The default background is grayscale.

The base image is always opaque. Overlays are rendered in CLI order. Values at or below an overlay's **display minimum** are transparent, transitioning gradually to its set opacity at the maximum. The atlas is always composited last, above all intensity layers.

## Atlas overlays

### Solid ID atlas (recommended)

```bash
niv mean_IF.nii.gz --axis x --rotate 90 \
  --atlas atlas_CCFv3_2020_30um.nii.gz \
  --lut CCFv3-2020_regional_summary.csv
```

The viewer generates boundaries between adjacent region IDs on each slice, located at **voxel faces**. This is a precise, unsmoothed wireframe, not a vector/spline renderer. The experimental smoothed raster contours from v0.5 did not improve appearance and were removed, along with `--atlas-style` and its `g` shortcut. The `--max-pixels` flag was also removed; it was an independent transmission-resolution cap, not a smoothing control. Image resolution now follows the terminal pixel/cell dimensions, which can increase bandwidth on large displays. `--fps 12` may help remote Rex connections.

LUT CSV columns: `Region_ID`, `Region`, `Abbr`, `R`, `G`, `B` (see [UNRAVEL reference](https://github.com/b-heifets/UNRAVEL/blob/dev/unravel/core/csvs/CCFv3-2020_regional_summary.csv)). Without a LUT, regions display only numeric IDs and the outline uses the fallback `--atlas-color`.

### Precomputed UNRAVEL wireframe

```bash
niv mean_IF.nii.gz \
  --atlas-wireframe atlas_CCFv3_2020_30um_W_IDs.nii.gz \
  --atlas-labels atlas_CCFv3_2020_30um.nii.gz \
  --lut CCFv3-2020_regional_summary.csv
```

Binary `_W` and ID-valued `_W_IDs` wireframes are both supported. Only the *solid label atlas* supplies region identity away from precomputed wireframe voxels; pass `--atlas-labels` for accurate hover lookup in this mode. Precomputed wireframe voxels are painted directly, not as between-voxel boundaries.

### Multiple intensity images

```bash
niv mean_IF.nii.gz --window 0,1200 \
  --overlay zstat_positive.nii.gz --overlay-window 2,6 --overlay-color ff6644 \
  --overlay zstat_negative.nii.gz --overlay-window 2,6 --overlay-color 4499ff \
  --atlas atlas_CCFv3_2020_30um.nii.gz \
  --lut CCFv3-2020_regional_summary.csv
```

`--overlay-window`, `--overlay-color`, `--overlay-opacity` apply to the preceding `--overlay`. All loaded 3D files must be on the **same voxel grid**. The tool checks dimensions, spacing and sforms, but does not resample or yet fully validate qforms. The X/Y/Z axes refer to raw NIfTI array indices, not affine-derived anatomical orientation.

## Other CLI options

See `niv --help`. Useful options include `--slice N`, `--step N`, `--rotate 90`, `--fps 12`, `--mouse-pixels`, `--no-enhanced-keys` and `--bench 100`. `NIV_AXIS`, `NII_AXIS` and `NIV_ROTATE` are supported. `--atlas-width 1` is the default pixel outline thickness.

Kitty graphics frames are encoded using RGB24 + zlib + base64. New frames are placed before the old placement is deleted, avoiding intentional blanking on normal slice changes; changing layout intentionally clears the screen once. Bandwidth and smoothness still depend on Rex and your terminal.

## Code structure

- `src/volume.rs`: NIfTI decoding, grid validation and scaling
- `src/render.rs`: nearest-neighbor slices, color maps, compositing, voxel-face contours
- `src/layers.rs`: layered images, palettes, atlas LUT and region lookup
- `src/app.rs`: navigation, fullscreen, layer selection and text-based range editor
- `src/main.rs`: terminal lifecycle, small status UI, scroll and mouse events
- `src/kitty.rs`: compressed Kitty image protocol
- `examples/`: demo NIfTI generators and Kitty test utility

MIT license. The upstream CCF LUT is maintained separately by UNRAVEL.
