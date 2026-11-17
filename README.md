# niv v0.8

Fast, keyboard-driven NIfTI stack viewer for Ghostty / Rex / Kitty-graphics terminals, written in Rust. Loads a `.nii` or `.nii.gz` volume into RAM, draws grayscale or color-mapped slices, composites additional aligned intensity layers, and optionally draws colored CCF atlas contours above them.

## Install / upgrade

```bash
cd ~/Documents/_GitHub/niv
# Extract v0.8 source-only update into this directory, or apply the patch if it fits your checkout.
cargo fmt
cargo test
cargo install --path . --force
niv --version       # v0.8.0
```

The full project ZIP contains `niv-rust/` as its top-level directory. The source-only ZIP contains `src/`, `Cargo.toml`, and `README.md` directly, ready to extract into your repository. Don't apply the patch *and* extract the ZIP. Save a Git commit before upgrading if you've made local edits.

## Viewer

```bash
niv brain.nii.gz                           # defaults to --axis x --rotate 90
niv brain.nii.gz --overlay activation.nii.gz
niv brain.nii.gz --background transparent # start in RGBA mode
niv brain.nii.gz --hover-delay-ms 250       # delay before thicker hovered-region contour
```

The screen has a colored region name at the upper left, a small `?` at upper right, voxel indices and intensity values for every layer on the next row, the image, and a **single selected-layer status line below the image**. It shows `1: image  |  min: …  |  max: …  |  α: …`. Names use a middle ellipsis if space is tight; `t` reveals a list of layers, while `I` shows full metadata. No persistent shortcut footer is shown. Loading messages/errors or typed range/opacity/path editors temporarily appear at the bottom.

### Keys

| Key | Function |
| --- | --- |
| `↑↓←→`, `hjkl`, mouse wheel | Scroll volume slices |
| Command + direction | ×2 where Rex/terminal forwards Command |
| Shift + direction | ×3 |
| Control + direction | ×4 |
| Option/Alt + direction | ×2 fallback for Command |
| `[` / `]` | Decrease/increase base stride by one |
| `b` | Toggle **black/RGB** ↔ **transparent/RGBA** background |
| `a` | Toggle atlas outlines |
| `?` or `/` | Popup keyboard help |
| `I` | Metadata and full layer names |
| `n` | Load another registered image below the atlas |
| `1`–`9`; `;` | Select layer; cycle beyond nine layers |
| `e`, `m`, `M` | Edit selected-layer range / only min / only max |
| `0` | Reset range to actual finite min/max |
| `o`; `O` | Edit selected-layer opacity; edit atlas opacity |
| `c` | Cycle gray → warm → cool → categorical integer-ID palette |
| `t` | Show/hide layer list |
| Space | Toggle selected overlay visibility |
| `i` | Keyboard inspection crosshair (arrows move probe) |
| `x`,`y`,`z` | Change array slicing axis |
| `r`,`R` | Rotate clockwise / counterclockwise |
| `f`,`v` | Flip horizontally / vertically |
| Tab | Hide/show viewer text, fill terminal |
| `q` | Quit (Esc dismisses popup/editor) |

**Important:** Command/Super may be reserved by Ghostty, Rex, or the OS and not arrive as a terminal key modifier. Option/Alt has the same ×2 mapping as a fallback. Shift/Ctrl/Option support likewise depends on which input sequences your Rex client forwards.

### How backgrounds, thresholds and opacity work

**Black / RGB (default)**: Each NIfTI layer is composited into an opaque black RGB canvas **in memory**. Values at/below a layer's display minimum are skipped, so that they reveal lower layers. Partial image/atlas opacity blends with lower layers (or black). Final empty voxels are black. The terminal receives Kitty `f=24` compressed RGB bytes (3 channels/pixel).

**Transparent / RGBA (`b`)**: Layers are composited in memory using straight alpha, with empty voxels remaining transparent. The terminal receives Kitty `f=32` (4 channels/pixel), allowing the Ghostty background to appear through empty pixels. Per-layer opacity and threshold behavior are the same in both modes. True final-terminal transparency requires RGBA, but **multiple-layer transparency does not**.

`NIV_BACKGROUND=black|transparent` or `--background black|transparent` sets the startup mode. Black is the faster default. Performance can differ due to compression ratios and terminal/Rex transmission; it is **not guaranteed** to be exactly 25% faster.

### Hovered-region highlight

With a **solid** atlas, entering a labeled region starts a short delay. If the pointer remains in that region past the delay, its contours are drawn thicker (3px) while other regions retain their usual outline thickness. Hover events *before* the delay only update the text line; they do not re-transmit the image. Moving between voxels of the same region does not restart the timer.

Use `NIV_HOVER_DELAY_MS=150` (default) or `--hover-delay-ms 150` to tune, 0–2000 ms; `0` disables the highlight. The thicker outline needs one full redraw when the region changes. For precomputed wireframe outlines, thickening requires companion solid labels (`--atlas-labels`); without them, the outline is unchanged. A keyboard probe remains available with `i` if iOS Rex does not forward mouse-move events.

### Environment variables

```zsh
export ATLAS="$HOME/Documents/__Heifets_lab_data/_UNRAVEL/atlases/CCFv3-2020/30um/atlas_CCFv3_2020_30um.nii.gz"
export ATLAS_LUT="$HOME/Documents/_GitHub/UNRAVEL/unravel/core/csvs/CCFv3-2020_regional_summary.csv"
export NIV_BACKGROUND=black
export NIV_HOVER_DELAY_MS=150
```

CLI `--atlas` / `--atlas-wireframe` and `--lut` override the environment. `--axis x --rotate 90` are defaults. Image and atlas NIfTI files must share identical dimensions, voxel spacing, and sform (when present); no resampling/reorientation is performed.

### Benchmark RGB vs RGBA

```bash
niv data/sample.nii.gz --background black --bench 100
niv data/sample.nii.gz --background transparent --bench 100
```

`--bench` reports **rasterization plus zlib encoding without terminal transmission**. Compare with actual scrolling through local and remote Rex to find whether CPU rendering, encoded bytes, or terminal transfer is the bottleneck. The application is currently CPU-rendered; it does **not** use multi-threading or a GPU. For the current CPU/Kitty pipeline, avoiding alpha bytes and unnecessary redraws is a low-complexity optimization worth testing before a GPU rewrite.

## Limits

- `float64` NIfTI data are converted to `f32` (hover shows seven *decimal places*, not guaranteed seven significant digits of source precision).
- True transparent pixels require Kitty RGBA support in the terminal/Rex client. Image opacity works in RGB mode too, but with black as the final background.
- The mouse hover outline is a rasterized 3-pixel contour, not a vector curve. Region contour topology remains voxel-accurate.
- Runtime `n` image loading and `.nii.gz` decompression are synchronous; loading large files can briefly block the UI.
- Hover events, Command modifier events and pixel-mouse tracking are client-dependent.
- `cargo test` and interactive visual testing should be run locally; this source distribution was assembled where Rust's toolchain was unavailable.
