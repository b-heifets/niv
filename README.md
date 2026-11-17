# niv

Keyboard-driven NIfTI slice viewer for Kitty-compatible terminals (Ghostty,
Rex, remote Rex/Tailscale), implemented in Rust. Volume data is loaded once;
per-frame work is nearest-neighbor rasterization, compositing, and compressed
Kitty graphics. No direct Rex or libghostty dependency.

**v0.5 — experimental.** This source release has not been compiled or tested
interactively in the build environment (no Rust toolchain or terminal graphics
available here). Run `cargo test`, then test inside Ghostty/Rex on your Mac.

## Install / update

From the project directory:

```sh
cargo test
cargo install --path . --force
niv --version  # 0.4.0
```

## Mouse hover and voxel inspector (v0.5)

Hover over the image to see the **raw (not windowed) voxel value of every image
layer**, array-space X/Y/Z indices, and the atlas's region abbreviation, full
name, and ID. The inspector uses the correct voxel across X/Y/Z, screen rotation,
and flips. Mouse movement only updates text, without retransmitting the image.
The terminal must forward mouse motion (SGR mouse tracking); iOS Rex touch
input may not generate hover events.

**Keyboard fallback** (also useful for precise inspection): press `i` to toggle
inspection mode. A small crosshair appears; arrows or `hjkl` move the inspector
one voxel in screen coordinates; Alt/Shift/Ctrl multiply movement by 2/5/10.
Use PgUp/PgDn to move 25 slices while inspecting, and `i` again to return
arrows/`hjkl` to slice scrolling. Mouse motion does not override the keyboard
crosshair while inspection mode is active.

Default mouse input is cell-based, so inspected position has terminal-cell
resolution. On terminals supporting **SGR 1016 pixel mouse coordinates**, you
can opt into finer precision:

```sh
niv data/sample.nii.gz --atlas data/demo_atlas.nii.gz \
    --lut data/demo_lut.csv --mouse-pixels
```

Try `--mouse-pixels` locally in Ghostty first. Some multiplexers and iOS
clients do not support forwarding pixel coordinates: if hovering stops working,
remove that flag. Mouse hover is still terminal-dependent, and no attempt is
made to guess which coordinate unit the mux actually forwarded.

If you use a *precomputed* wireframe, the outline volume may be zero in the
interior of a labeled region, so it cannot provide reliable region names.
Pass a matching solid label atlas only for inspection:

```sh
niv mean_IF.nii.gz \
  --atlas-wireframe atlas_CCFv3_2020_30um_W_IDs.nii.gz \
  --atlas-labels atlas_CCFv3_2020_30um.nii.gz \
  --lut CCFv3-2020_regional_summary.csv
```

The label volume is loaded once into RAM; its voxels are never painted over
the grayscale image. The atlas LUT is required for abbreviations/names (without
it you see only numeric IDs). All volumes must have matching array grids.

## Smooth atlas outlines (v0.5)

Solid label atlas outlines default to `--atlas-style smooth`. This mode
extracts boundaries on voxel **faces**, paints 1px antialiased strokes, and
rounds ordinary 90° contour corners by 0.22 source voxels using short
quadratic curves. This is a fast *raster* approximation to vector curves,
not a high-resolution spline fitted to atlas anatomy. Multi-region junctions
are deliberately not rounded, and small labeled areas are retained.

```sh
niv mean_IF.nii.gz --atlas atlas_CCFv3_2020_30um.nii.gz \
  --lut CCFv3-2020_regional_summary.csv --atlas-style smooth \
  --max-pixels 800000
```

Press `g` to switch between **smooth** and **voxel** while running. Voxel mode
uses the precise unsmoothed boundaries from v0.4 and should be preferred when
preserving exact segmentation geometry is more important than aesthetics.
For local Ghostty you can try a larger `--max-pixels` for finer anti-aliasing;
for remote Rex, use a lower limit (e.g. `120000`) and `--fps 12`.

Precomputed `_W` and `_W_IDs` wireframes remain voxel-mask rendering even if
`--atlas-style smooth` is requested: a wireframe mask does not contain enough
label topology to recover curved **between-voxel** contours. For smooth lines,
load the solid label atlas with `--atlas`.

### Solid label atlas — recommended

Given a background NIfTI and an identically registered Allen CCFv3 2020
label atlas, compute the **outline on the fly**, on 2D voxel *faces* (not
voxel centers). No precomputed wireframe is required:

```sh
niv mean_IF.nii.gz --axis x --rotate 90 \
  --atlas atlas_CCFv3_2020_30um.nii.gz \
  --lut CCFv3-2020_regional_summary.csv \
  --atlas-width 1
```

The LUT file is the [UNRAVEL CCFv3-2020 regional summary CSV](https://github.com/b-heifets/UNRAVEL/blob/dev/unravel/core/csvs/CCFv3-2020_regional_summary.csv).
It uses `Region_ID` (the atlas NIfTI voxel value), `Region`, `Abbr`, and
`R,G,B` channels. The CSV is **not bundled** with this project; use your
existing UNRAVEL copy or download it:

```sh
curl -fL https://raw.githubusercontent.com/b-heifets/UNRAVEL/dev/unravel/core/csvs/CCFv3-2020_regional_summary.csv -o CCFv3-2020_regional_summary.csv
```

`--atlas-color ffffff` is the fallback color if no LUT is supplied or an ID
is absent from it. `--atlas-width 1` draws the thinnest contours; 2 or 3 are
available for very small terminal views.

### Precomputed wireframe

Both output types from UNRAVEL's [`atlas_wireframe` (`wf`)](https://github.com/b-heifets/UNRAVEL/blob/dev/unravel/image_tools/atlas/wireframe.py)
are accepted:

```sh
# Recommended: ID-valued wireframe. Color each nonzero boundary voxel via LUT.
niv mean_IF.nii.gz --atlas-wireframe atlas_CCFv3_2020_30um_W_IDs.nii.gz --lut CCFv3-2020_regional_summary.csv

# Binary wireframe: all nonzero voxels get the same fallback color.
niv mean_IF.nii.gz --atlas-wireframe atlas_CCFv3_2020_30um_W.nii.gz --atlas-color ffffff
```

A precomputed wireframe is already a **voxel mask**, so lines from this
mode occupy the wireframe voxels. To draw boundaries *between* voxels as
requested, prefer `--atlas` with the full solid label atlas.

### Multiple intensities under the atlas

Additional volumes are overlaid **in command-line order** above the grayscale
base, *below* the atlas. Intensities at or below each overlay's lower display
limit are fully transparent. Values above the lower limit become progressively
more opaque until the upper limit, when they reach `--overlay-opacity`.

```sh
niv mean_IF.nii.gz --axis x --rotate 90 --window 0,1200 \
  --overlay zstat_positive.nii.gz --overlay-window 2,6 --overlay-color ff6644 --overlay-opacity 0.80 \
  --overlay zstat_negative.nii.gz --overlay-window 2,6 --overlay-color 4499ff --overlay-opacity 0.75 \
  --atlas atlas_CCFv3_2020_30um.nii.gz --lut CCFv3-2020_regional_summary.csv
```

Each `--overlay-window`, `--overlay-color`, and `--overlay-opacity` modifies
**the most recently declared `--overlay`**. Overlay windows are static in v0.4;
the interactive `+/-` and `,/.` controls still adjust the base grayscale
window only. All volumes must be voxel-aligned. Mismatched dimensions,
pixel spacing, or sform transforms are rejected with an error rather than
silently drawn out of register. When NIfTI sform is missing, the tool **cannot
fully verify qform alignment**: check registration yourself. No resampling or
world-coordinate anatomical orientation is implemented yet.

### Try the included aligned demo

Included `data/sample.nii.gz`, `data/demo_atlas.nii.gz`,
`data/demo_activation.nii.gz`, and `data/demo_lut.csv` all share the same
NIfTI grid. Test without any lab data:

```sh
niv data/sample.nii.gz --axis z \
  --overlay data/demo_activation.nii.gz --overlay-window 0.5,4.0 --overlay-color ff6644 \
  --atlas data/demo_atlas.nii.gz --lut data/demo_lut.csv
```

To regenerate all these sample files without third-party Python packages:

```sh
cd data
python3 ../examples/make_sample.py
cd ..
python3 examples/make_layer_demo.py
```

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| Arrows / `hjkl` | Scroll through slices |
| Alt / Shift / Ctrl + navigation | ×2 / ×5 / ×10 slice motion |
| `[` / `]` | Halve / double base slice stride |
| `x`, `y`, `z`, `a` | Select raw NIfTI array axis / cycle |
| `r` / `R` | Rotate display clockwise/counterclockwise by 90° |
| `f` / `v` | Flip horizontally/vertically |
| `b` | Show/hide topmost atlas outline |
| `g` | Toggle smooth/voxel contour style (solid atlas only) |
| Mouse hover | Show raw value of base/each overlay and atlas region |
| `i` | Keyboard inspector on/off (arrows/hjkl move crosshair) |
| `o` | Show/hide all additional intensity overlays |
| `+` / `-` | Narrow / widen base grayscale window |
| `,` / `.` / `0` | Shift base grayscale window / reset |
| PgUp/PgDn, Home/End | Jump through slices |
| `q` / Esc / Ctrl+C | Quit |

`--axis`, `--rotate`, and the `NIV_AXIS`, `NIV_ROTATE` environment variables
are supported as in v0.3. `NII_AXIS` is a fallback for `NIV_AXIS`. `--fps 12
--max-pixels 120000` can help remote sessions. `--bench 100` renders frames
without a terminal and includes overlays in its timing.

## Rendering details and limitations

- Solid label atlas: scan adjacent 2D labels, project faces between voxels into
  display pixels, draw one-pixel colored lines **last**. This is fast for a
  single plane and keeps the outline visible above all image layers.
- Precomputed atlas: draw nonzero mask voxels after image overlays. A wireframe
  `_W_IDs` atlas uses those voxel values as LUT lookup IDs.
- No full-screen clearing on normal slice changes: fresh Kitty image placement
  is transmitted before the old frame is deleted (same behavior as v0.3).
- The status displays the data and grayscale display range, layer counts, and
  layer-by-layer intensity, and full region text at the hovered or keyboard-
  inspected voxel.
- The loader uses `ndarray::Array3<f32>` for each 3D NIfTI; a 440×267×380
  image occupies ~170 MiB **per volume** after decompression. Loading two
  additional intensities and an atlas uses roughly four times the base image
  memory, plus temporary allocations.
- Grayscale base image is never transparent (black at/below the lower window
  limit). Additional intensity layers are transparent at/below their own
  lower bounds. Colors are composited into one RGB frame to avoid Kitty
  multi-placement synchronization issues over Rex.
- The voxel indices correspond to array X/Y/Z, **not affine-derived
  anatomical axes**. Rotation is display-only. Atlas alignment must be
  established in voxel coordinates before loading.
- The LUT colors are literal region RGB channels from the CSV; there is no
  hierarchy filtering, region-name search, or adjustable alpha for atlas lines
  in this first version.

## Code layout

- `src/main.rs`: terminal state, mouse input, low-overhead inspector footer, status and benchmark
- `src/config.rs`: CLI and layer options
- `src/volume.rs`: NIfTI loading and strict matching-grid checks
- `src/layers.rs`: overlay and atlas structures; CSV LUT parsing
- `src/render.rs`: nearest-neighbor mapping, alpha compositing, smoothed or exact voxel-face contours, cursor mapping
- `src/kitty.rs`: zlib+base64 Kitty graphics protocol
- `src/app.rs`: keyboard controls and toggles
- `examples/make_layer_demo.py`: generate aligned demo volumes

Licensed MIT; CCF color CSV is provided separately by UNRAVEL.
