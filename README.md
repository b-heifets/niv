# niv

Fast keyboard-driven 3D NIfTI slice viewer for Kitty graphics terminals, including Ghostty and Rex. This v0.7 source release adds true RGBA transparency, independent image and atlas opacity, runtime overlays, and help/info panels.

**Build status:** source and test-data structure were checked, but this release has not yet been compiled or interactively tested in a Kitty terminal here. Run the commands below on macOS.

## Build

```bash
cd ~/Documents/_GitHub/niv
cargo fmt --check
cargo test
cargo install --path . --force
```

For a transmission-free render benchmark:

```bash
niv data/sample.nii.gz --bench 50
```

## Quick start

```bash
niv image.nii.gz                                # default: --axis x --rotate 90
niv image.nii.gz --atlas other_atlas.nii.gz
niv image.nii.gz --overlay activation.nii.gz
```

Set defaults in `~/.zshrc` (paths are examples; change them to match your computer):

```zsh
export ATLAS="$HOME/Documents/__Heifets_lab_data/_UNRAVEL/atlases/CCFv3-2020/30um/atlas_CCFv3_2020_30um.nii.gz"
export ATLAS_LUT="$HOME/Documents/_GitHub/UNRAVEL/unravel/core/csvs/CCFv3-2020_regional_summary.csv"
```

`--atlas` or `--atlas-wireframe` overrides `$ATLAS`; `--lut` overrides `$ATLAS_LUT`. The atlas must have the same voxel grid as the base image. If this isn't the case, resample externally; `niv` does **not** resample or anatomically reorient images yet.

The help commands are deliberately different: `niv -h` prints a short guide, while `niv --help` prints comprehensive options and keybindings.

## Default display

The viewer shows one bold, CCF-LUT-colored region line above the image (no `Atlas:` prefix). The region under the mouse/crosshair is shown, or the middle voxel if no probe is active. Below the image is the selected intensity layer, its editable min/max display range, and its opacity; the next line shows voxel coordinates and each layer's original intensities (7 decimal places). Only a dim `? help` hint remains at the bottom.

The image and status text are horizontally aligned. Press `Tab` for image-only mode. Press `?` or `/` for a centered help sheet, or `I` for metadata (dimensions, voxel spacing, NIfTI datatype, exact data range, layer metadata, atlas state). The sheet temporarily hides the graphics placement to remain readable in terminal multiplexers; `Esc` restores the image.

### Common keys

| Key | Action |
| --- | --- |
| arrows, `hjkl`, wheel/two-finger trackpad | Scroll slices |
| Shift / Ctrl / Alt + direction | ×2 / ×3 / ×4 |
| `[` / `]` | Decrease / increase stride by 1 |
| `1`–`9` | Select image layer (1 = base) |
| `;` | Cycle selected layer, including layers beyond 9 |
| `e` | Enter selected image display `min,max` |
| `m` / `M` | Enter display minimum / maximum |
| `0` | Reset range to full finite data min/max |
| `o` | Enter selected image opacity from 0 to 1 |
| `O` | Enter atlas opacity from 0 to 1 |
| `a` | Toggle atlas |
| `n` | Enter another NIfTI image path; load below atlas |
| `Space` | Toggle selected overlay |
| `c` | Cycle gray → warm → cool → categorical palette |
| `t` | Toggle layer list |
| `Tab` | Image-only / normal view |
| `?` or `/` | Open/close help |
| `I` | Open/close volume information |
| `i` | Keyboard voxel inspector |
| `q` | Quit |

The help sheet includes additional keys (`r/R`, `x/y/z`, `f/v`, contrast and brightness). While an editor is open, `Enter` confirms and `Esc` cancels. For `n`, enter a plain file path (relative to current directory, absolute, or starting `~/`); shell auto-completion isn't implemented.

## Image and atlas rendering

- The base volume and every overlay are decoded once, retained as `f32` in RAM, and sampled nearest-neighbor.
- The **default display range is the exact finite data min/max**, including zero and negative values. For constant-valued images the window is widened by 0.5 on either side to remain valid.
- Voxels with intensity **at or below the selected display minimum are fully transparent** for the base and for all overlays, regardless of palette. This is real Kitty RGBA (`f=32`), not painting a solid background. Above minimum, image opacity is constant; colors are clipped at maximum.
- Layer order is base → overlays in loading order → atlas. Atlas wireframe is always on top. `o` edits the selected image opacity, `O` the atlas opacity.
- The **categorical** color scale maps each *rounded integer intensity* deterministically to its own saturated hue. It is designed for cluster IDs/label maps, **not** smooth scalar data; distinct integers produce stable hues across slices and layers.
- Gray scales from black to white; warm from red to yellow; cool from blue to cyan. `c` cycles these followed by categorical. A custom `--overlay-color RRGGBB` begins in fixed-tint mode.
- Atlas colors and region text come from the UNRAVEL CCFv3-2020 LUT (`Region_ID,Region,Abbr,R,G,B`). Solid atlas contours are generated from neighboring labels at voxel faces. `--atlas-wireframe` supports precomputed masks, and `--atlas-labels` allows accurate region lookup in that case.
- Hovering updates only the text inspector. If iOS Rex cannot forward hover, press `i` and move the probe with arrow keys. Mouse wheel/trackpad scrolling requires wheel events to be forwarded by the terminal and Rex.
- Raw `f32` intensities are shown with seven decimal places; this is display formatting, **not** a guarantee of seven-digit precision in source data that were stored at lower precision or converted from float64.

## CLI examples

```bash
# Atlas and LUT from environment (defaults x axis, 90° rotation)
niv mean_IF.nii.gz

# Atlas from CLI, plus colored overlays
niv mean_IF.nii.gz \
  --overlay positive.nii.gz --overlay-window 2,6 --overlay-color ff3333 \
  --overlay negative.nii.gz --overlay-window 2,6 --overlay-color 2288ff \
  --atlas labels.nii.gz --lut CCFv3-2020_regional_summary.csv

# Initial transparency from command line
niv mean_IF.nii.gz --opacity .75 --atlas-opacity .6

# UI-free benchmark
niv mean_IF.nii.gz --bench 100
```

The included `data/sample.nii.gz`, `data/demo_atlas.nii.gz`, `data/demo_activation.nii.gz`, and `data/demo_lut.csv` are aligned synthetic test inputs. The full CCFv3 LUT is **not bundled**.

## Limitations

This is a terminal prototype, not an affine-aware anatomical viewer. NIfTI data are converted to `f32`, so `float64` inputs lose some precision. Both terminal/Rex Kitty RGBA support and interactive mouse/trackpad behavior must be tested on your specific clients. Image loading through `n` is synchronous (there may be a pause for a large `.nii.gz`). The centered help/info sheet temporarily hides the image rather than compositing text above it, to avoid Kitty z-order incompatibilities in multiplexers. The non-vector voxel boundary appearance is unchanged.
