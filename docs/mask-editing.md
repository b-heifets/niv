# Interactive NIfTI editing (`niv edit`)

`niv` provides a **non-destructive** spherical segmentation editor for correcting registration-preparation autofluorescence images. It works in original voxel-index coordinates; no automatic RAS canonicalization, resampling, or reorientation is performed. This experimental editor is not a full 3D Slicer replacement.

## Enter from the viewer

```sh
niv autofluo_50um_masked.nii.gz
# Press Ctrl+E in the normal viewer to enter Edit mode.
```

`Ctrl+E` edits the image originally loaded as the reference volume. The viewer remains open and is restored when you leave the editor with `q`. Original file contents remain unchanged; saved derived files can be loaded separately using the normal viewer's `n` picker.

Alternatively, start in Edit mode and optionally provide an existing brain mask or original autofluorescence reference:

```sh
niv edit autofluo_50um_masked.nii.gz
niv edit autofluo_50um_masked.nii.gz --mask autofluo_50um_brain_mask.nii.gz --raw autofluo_50um.nii.gz
```

The previous `--masked IMAGE` syntax also works. `--mask` and `--raw` require matching voxel grids. No segmentation is automatically created. The unmodified input is displayed in grayscale; press `n` for a **Trim** (green) segmentation or `N` for a **Fill** (cyan) segmentation. Name it when prompted (Enter accepts its generated default).

Trim segmentations assign zero intensity to the painted voxels and remove them from the derived brain mask. Fill segmentations assign a user-defined intensity and mark those voxels as brain. Painting uses a **3D physical sphere** respecting voxel spacing (not a flat 2D disk). Multiple segments can have different fill intensities and colors. Later segments take precedence where they overlap.

## Editing controls

| Control | Action |
|---|---|
| `Ctrl+E` (normal viewer) | Enter editor using currently loaded reference image |
| `z`, `x`, `c` or `y` | Focus Z, X, or Y orthoplane (same mapping as viewer) |
| `Z`, `X`, `C` or `Y` | Show/hide the corresponding orthoplane; at least one stays visible |
| `1`, `2`, `3` / `Tab` | Focus X/Y/Z directly or cycle visible planes |
| `f` | Toggle focused view fullscreen / multiview |
| Up/Down or mouse wheel | Scroll focused plane, using the chosen stride |
| `[` / `]` | Reduce/increase slice stride |
| Shift + mouse wheel | Reduce/increase 3D brush diameter (50 µm increments) |
| `,` / `.` | Reduce/increase brush diameter (25 µm increments) |
| Ctrl + mouse wheel, `+` / `-` | Zoom **only the focused plane** |
| `0` | Reset focused-plane zoom |
| Shift + arrow keys | Pan the focused plane (especially useful when zoomed) |
| Shift + left-drag | Pan the focused plane |
| Shift + hover | Move orthogonal crosshair intersections without painting |
| Left-drag / right-drag | Paint / erase a 3D sphere |
| `p` / `e`, then Space | Paint / erase at crosshair with keyboard |
| `n` / `N` | Create and name new Trim / Fill segment |
| `l` | Open segment list; Up/Down select, Space toggle visibility, Enter/Esc close |
| `;` or `S` | Select next segment |
| `r` / `V` | Rename / toggle visibility of selected segment |
| `o` / `O` | Decrease / increase segmentation opacity (default 22%) |
| `i` / `I` | Sample / enter constant fill intensity for selected Fill segment |
| `m` | Toggle display of derived binary brain mask |
| `B` | Enable/disable saving a binary brain mask |
| `u` or Cmd+Z | Undo brush stroke |
| `U` or Ctrl+R | Redo brush stroke |
| `s` | Save derived NIfTI(s) and all segmentation masks |
| `?` | Open in-editor help; Esc or `?` closes it |
| `q` | Exit editor (requires second press if unsaved); returns to viewer when entered with Ctrl+E |

**Terminal caveat:** Cmd+Z, Shift+hover, and Shift+wheel depend on keyboard/mouse modifier events from Ghostty/Rex. The simple `u`, `U`, `,`, `.`, and arrow-key alternatives remain available.

## Saving and mask derivation

The default save location is a new directory next to the input (`autofluo_50um_masked_niv_edits/`, with numeric suffix if needed). `s` writes the derived intensity image, every segmentation as an individual binary `.nii.gz`, and a `.nii.gz.toml` sidecar containing its role, color, visibility and fill intensity. Source images are never overwritten.

When `--mask` is provided, an edited brain mask is saved by default. **When no brain mask was loaded**, an optional mask can be created from positive finite intensities of the input (`>0`), then altered by the paint/erase segments. Press `B` to enable mask export before saving, or start the CLI with `--binarize-mask`. Mask export can also be turned off with `B`.

```sh
niv edit autofluo_50um_masked.nii.gz --binarize-mask --out registration_edits
```

A typical output is:

```text
autofluo_50um_masked_niv_edits/
├── autofluo_50um_masked_niv_edited.nii.gz
├── autofluo_50um_masked_niv_brain_mask.nii.gz    # only if exporting mask
├── 01_exterior_trim.nii.gz
├── 01_exterior_trim.nii.gz.toml
├── 02_tissue_fill.nii.gz
└── 02_tissue_fill.nii.gz.toml
```

Continue editing by supplying previously saved `--trim FILE.nii.gz` and `--fill FILE.nii.gz` arguments. Sidecars restore colors, visibility, and fill intensities. Reopen the **original** autofluorescence image to avoid applying saved segments twice. Add `--overwrite` only when intentionally updating existing output files.

## Limitations and verification

- The initial implementation has a maximum of 64 stroke-level undo operations. Undo history is not persisted.
- Saved overlays are binary segmentations; the current editor supports one intensity image at a time, plus optional raw comparison.
- `B` derives the brain mask with a `>0` threshold rather than sophisticated tissue segmentation. Verify it against the original brain mask if you have one.
- NIfTI headers are used as write references to preserve the spatial metadata; the written datatype matches the output volume. Always verify header, voxel alignment, intensity values, and saved masks before registration.
- Large inputs are loaded in memory. For full-resolution LSFM, use existing image viewers/converters for now.
- `NIV_EDIT_PIXEL_BUDGET` sets the image raster budget (default 1,000,000). This does not affect source voxel precision.
