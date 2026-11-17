# niv — environment defaults for zsh
# Install at ~/.config/niv/env.zsh and source it from ~/.zshrc.
# Existing environment values take precedence; CLI flags override these defaults.

# Default slice orientation
export NIV_AXIS="${NIV_AXIS:-x}"
export NIV_ROTATE="${NIV_ROTATE:-90}"

# Renderer: black = faster RGB; transparent = terminal-background RGBA.
# Press b in niv to switch modes without restarting.
export NIV_BACKGROUND="${NIV_BACKGROUND:-black}"

# Hovered atlas-region outline delay (milliseconds; 0 disables highlight)
# Disabled for maximum scrolling performance. Set to 150 to re-enable.
export NIV_HOVER_DELAY_MS="${NIV_HOVER_DELAY_MS:-0}"

# Raster resolution budget; 0 = full native-resolution terminal pixels.
export NIV_PIXEL_BUDGET="${NIV_PIXEL_BUDGET:-200000}"

# Frame-rate cap helps reduce Rex/Ghostty graphics backlog (1..120)
export NIV_FPS="${NIV_FPS:-30}"

# Antialias solid-atlas contours: off by default for fast scrolling.
# Press g in niv to toggle temporarily; --atlas-aa enables for one run.
export NIV_ATLAS_AA="${NIV_ATLAS_AA:-0}"

# Allen CCFv3 2020 atlas and regional-color LUT
# Adjust ATLAS_LUT if UNRAVEL is checked out somewhere else.
export ATLAS="${ATLAS-$HOME/Documents/__Heifets_lab_data/_UNRAVEL/atlases/CCFv3-2020/30um/atlas_CCFv3_2020_30um.nii.gz}"
export ATLAS_LUT="${ATLAS_LUT-$HOME/Documents/_GitHub/UNRAVEL/unravel/core/csvs/CCFv3-2020_regional_summary.csv}"

# Note: automatic atlas loading requires the viewed NIfTI to share the atlas grid.
# Override one invocation with: ATLAS= niv some_other_space.nii.gz
# Per-run CLI flags still override these defaults. Atlas opacity and
# --mouse-pixels are CLI-only settings.
