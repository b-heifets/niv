# niv

Keyboard-driven NIfTI slice viewer for Kitty-compatible terminals (Ghostty,
Rex, remote Rex/Tailscale), implemented in Rust. Volume data is loaded once;
per-frame work is nearest-neighbor rasterization, compositing, and compressed
Kitty graphics. No direct Rex or libghostty dependency.

**v0.4 — experimental.** This source release has not been compiled or tested
interactively in the build environment. Please run `cargo test` on your Mac.

## Install / update

From the project directory:

```sh
cargo test
cargo install --path . --force
niv --version  # 0.4.0
```

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
  label/abbreviation at the **center voxel of the solid atlas's current plane**.
  No movable crosshair or per-voxel hover inspector yet.
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

- `src/main.rs`: terminal state, event loop, status and benchmark
- `src/config.rs`: CLI and layer options
- `src/volume.rs`: NIfTI loading and strict matching-grid checks
- `src/layers.rs`: overlay and atlas structures; CSV LUT parsing
- `src/render.rs`: nearest-neighbor mapping, alpha compositing, between-voxel contours
- `src/kitty.rs`: zlib+base64 Kitty graphics protocol
- `src/app.rs`: keyboard controls and toggles
- `examples/make_layer_demo.py`: generate aligned demo volumes

Licensed MIT; CCF color CSV is provided separately by UNRAVEL.
