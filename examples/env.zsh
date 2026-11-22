# Optional niv defaults. Copy to ~/.config/niv/env.zsh and source from ~/.zshrc.
# The atlas, LUT, and template remain opt-in even when defined here.
# Raw voxel-index view by default. Use ccf for X/90 instead.
# No sform/qform reorientation is applied in either case.
export NIV_VIEW_PRESET=raw
# Optional individual overrides; uncomment if desired:
# export NIV_AXIS=z
# export NIV_ROTATE=0
export NIV_BACKGROUND=transparent
export NIV_PIXEL_BUDGET=1000000
export NIV_MONTAGE_PIXEL_BUDGET=4000000
export NIV_HOVER_DELAY_MS=20
export NIV_FPS=30

# Independent input directions. Normal preserves niv's existing behavior.
export NIV_SCROLL_KEYS=normal
export NIV_SCROLL_WHEEL=normal

# Configure these for your own atlas and reference images.
# export ATLAS='/path/to/atlas.nii.gz'
# export ATLAS_LUT='/path/to/atlas_regions.csv'
# export TEMPLATE='/path/to/template.nii.gz'

# Optional: alternate keymap location (otherwise ~/.config/niv/keys.toml).
# export NIV_KEYS="$HOME/.config/niv/keys.toml"

# Navigation modifiers: Shift ×4, Ctrl ×3, Option/Alt ×2;
# Command/Super has no multiplier. These are built-in key defaults.
