# Interactive segmentation editor (`niv` v0.30.0, experimental)

`niv` can paint 3D spherical **segmentations** over NIfTI images in three synchronized orthogonal views. Painting is non-destructive: the original intensity image remains visible under translucent segmentations, and an optional derived output preview shows the intensity modification. Voxel arrays are never reoriented or resampled.

## Start and load images

```bash
niv                                       # open NIfTI picker
niv autofluo_50um_masked.nii.gz          # Ctrl+E to enter edit mode
niv edit autofluo_50um_masked.nii.gz     # open editor directly
niv edit autofluo_50um_masked.nii.gz --mask autofluo_50um_brain_mask.nii.gz
```

Press **Ctrl+E** from normal viewing to edit the currently selected image, retaining its slice axis, slice index, and display range. Press **q** to return (twice if there are unsaved edits).

Inside the editor, **`n`** opens a searchable NIfTI picker to add another aligned image, such as `autofluo_50um.nii.gz`. Select the displayed image by clicking its name in the bottom image bar, or cycle with **`\`**. Additional images must have the same voxel grid; they are display/reference images and do not replace the original image being edited. Each image has an independent grayscale display window.

## Make a segmentation

Press **`a`** to create and name a segmentation. Nothing is created automatically. The first segmentation is green; subsequent ones use other colors, and all start at 22% opacity. Paint the segmentation with the left mouse button and erase with the right mouse button. The brush is a physical 3D sphere, accounting for anisotropic voxel spacing.

Creating or painting a segmentation **does not change output intensity values by itself**. To decide how the segment affects the saved intensity image, press **`t`** to cycle:

- **Off:** segment is a visual and exportable label mask only.
- **Inside:** set voxel intensity *inside* painted voxels to the chosen target value.
- **Outside:** set voxel intensity *outside* painted voxels to that value.

Use **`I`** to enter an exact target intensity, or **`=`** to sample the value near the cursor. The initial target intensity is 0, so `t` once enables zeroing inside a segmentation (trimming); set a nonzero intensity to fill missing tissue. **Later segments take precedence** when intensity operations overlap. Press **`p`** to switch between the original grayscale image (the default while painting) and the proposed edited output. The color overlay remains translucent in both modes.

Segments are independent masks, no longer intrinsically named Trim or Fill. Older `--trim` and `--fill` files are still supported, and previous TOML sidecars remain readable.

## Keybindings

| Control | Action |
|---|---|
| `Tab` / Shift+Tab | Cycle focused view forward / backward |
| `z`, `x`, `c` or `y` | Toggle that orthogonal view's visibility (case-insensitive) |
| `f` or Enter | Show only focused view; press again to show all three |
| Up/Down / mouse wheel | Scroll focused view, respecting stride |
| Left/Right | Move linked plane index using the same stride |
| `[` / `]` | Decrease / increase stride |
| Shift+arrows | Pan selected view by the configured stride |
| Middle-button drag, Shift+left-drag | Pan the corresponding view |
| Ctrl+wheel, `+` / `-` | Zoom selected view in smaller 0.1× steps |
| `0` | Reset focused view zoom and center |
| `r`, `h`, `v` | Rotate 90°, flip horizontally, flip vertically (focused view only) |
| Shift+hover | Update crosshair positions without painting (voxel-precise) |
| Left-drag / right-drag | Paint / erase a 3D sphere |
| Shift+wheel, `,` / `.` | Adjust brush diameter |
| `a`, F2 | New named segmentation / rename selected segment |
| `1`–`9` | Select a segment by number |
| Shift+`1`–`9` | Toggle visibility of numbered segment (also works with `!@#...`) |
| `;` | Cycle to next segment |
| `l` | Segmentation menu (arrows select, Space show/hide) |
| `o` / `O` | Decrease / increase segmentation opacity |
| `t`, `I`, `=` | Cycle segment operation; set value; sample value |
| `e`, then Space | Toggle keyboard paint/erase brush, apply at crosshair |
| `n` | Load additional aligned NIfTI with in-app picker |
| `\` or click image bar | Cycle / select displayed image |
| `m` / `M`, `w` | Edit image min / max; or both together |
| `i` | Selected image information, including dimensions and spacing |
| `p` | Toggle edited output preview for the base image |
| `u`, `Ctrl+Z`, `Cmd+Z` | Undo paint stroke |
| `U`, `Ctrl+R`, `Ctrl+Shift+Z`, `Cmd+Shift+Z` | Redo paint stroke |
| `b` / `B` | Toggle loaded mask display / binary brain-mask export |
| `s` | Save derived image, optional mask, and all segmentations |
| `?` | Centered, sectioned help menu |
| `q` | Return to normal view, with unsaved-changes warning |

**Mouse modifier note:** Whether Shift+hover and Shift+drag reach the application depends on the terminal and OS mouse handling. Middle-drag and keyboard panning are alternatives.

## Saving and resuming

By default, `s` creates a new directory next to the source image. Files include an edited copy of the source, every named binary segmentation, and a `.toml` sidecar per segment with its color, visibility, target intensity, and **off/inside/outside** operation. The source image is never overwritten.

Press **`B`** before saving to write a binary brain mask as well. The baseline mask is either your optional `--mask` image or a binary image from positive finite intensity values of the original image. Segmentation operations are applied to both the intensity copy and binary mask, setting the mask to 1 for positive output intensities and 0 for zeroed intensities. This is an output convenience, not anatomical segmentation; check the result against your original brain mask.

For example:

```text
autofluo_50um_masked_niv_edits/
├── autofluo_50um_masked_niv_edited.nii.gz
├── autofluo_50um_masked_niv_brain_mask.nii.gz  # only with B/--binarize-mask
├── 01_trim_exterior.nii.gz
├── 01_trim_exterior.nii.gz.toml
├── 02_fill_gap.nii.gz
└── 02_fill_gap.nii.gz.toml
```

Resume by reopening the **original source image** and supplying saved segment paths with `--segment` (repeatable):

```bash
niv edit autofluo_50um_masked.nii.gz \
  --segment autofluo_50um_masked_niv_edits/01_trim_exterior.nii.gz \
  --segment autofluo_50um_masked_niv_edits/02_fill_gap.nii.gz
```

Older `--trim` and `--fill` inputs and metadata remain supported. Do not reopen the derived edited image and reapply the same segment operations: this would apply those operations twice. Use `--overwrite` intentionally if updating files in an existing output directory.

## Current limitations

The editor displays one grayscale image at a time rather than compositing the normal viewer's full layer stack. It cannot create or merge more advanced Slicer segmentation effects, and it does not yet save a complete interactive editor session (view orientations, crosshair, zoom, and window settings) separately. Intensity operations are constant-value replacements without interpolation from surrounding voxels. The initial `niv` file picker and edit picker support directory browsing and text filtering; some normal viewer picker conveniences are not yet implemented in the editor picker.

All segments can be exported even if their intensity operation is off. Brush undo history is limited to 64 strokes and isn't saved between sessions. Validate intensity values, NIfTI metadata, orientations, binary masks, and saved segment alignment on copies before using them for registration.
