use std::path::Path;

use anyhow::{bail, Context, Result};
use ndarray::{Array3, Axis as NdAxis, Ix3};
use nifti::{IntoNdArray, NiftiObject, ReaderOptions};

/// Array index axes, NOT anatomical RAS/LPS or physical axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "x" => Ok(Self::X),
            "y" => Ok(Self::Y),
            "z" => Ok(Self::Z),
            _ => bail!("Unknown axis {s:?}; expected x, y, or z"),
        }
    }
    pub fn index(self) -> usize {
        match self { Self::X => 0, Self::Y => 1, Self::Z => 2 }
    }
    pub fn label(self) -> &'static str {
        match self { Self::X => "X", Self::Y => "Y", Self::Z => "Z" }
    }
    pub fn cycle(self) -> Self {
        match self { Self::X => Self::Y, Self::Y => Self::Z, Self::Z => Self::X }
    }
    pub fn plane_axes(self) -> (usize, usize) {
        match self {
            Self::X => (1, 2), // horizontal index Y; vertical index Z
            Self::Y => (0, 2), // horizontal index X; vertical index Z
            Self::Z => (0, 1), // horizontal index X; vertical index Y
        }
    }
}

pub struct Volume {
    pub data: Array3<f32>,
    pub spacing: [f32; 3],
}

impl Volume {
    pub fn load(path: &Path) -> Result<Self> {
        let object = ReaderOptions::new().read_file(path)
            .with_context(|| format!("Failed to read NIfTI file: {}", path.display()))?;
        // Reject 4D/5D acquisitions before converting all voxels to f32.
        let dims = object.header().dim;
        let rank = dims[0] as usize;
        if !(3..=7).contains(&rank) || (4..=rank).any(|axis| dims[axis] != 1) {
            bail!("Only 3D NIfTI volumes are supported (header dimensions {:?})", dims);
        }
        let mut spacing = [1f32; 3];
        for (axis, s) in spacing.iter_mut().enumerate() {
            let value = object.header().pixdim[axis + 1].abs();
            if value.is_finite() && value > 0.0 { *s = value; }
        }
        // nifti's into_ndarray applies scl_slope/scl_inter, then casts to f32.
        // The resulting ndarray usually retains NIfTI's Fortran-order layout.
        let mut dynamic = object.into_volume().into_ndarray::<f32>()
            .context("Failed to decode voxel data")?;
        while dynamic.ndim() > 3 {
            let last = dynamic.ndim() - 1;
            if dynamic.shape()[last] != 1 {
                bail!("Expected a 3D volume or singleton higher dimensions; got {:?}", dynamic.shape());
            }
            dynamic = dynamic.index_axis_move(NdAxis(last), 0);
        }
        if dynamic.ndim() != 3 { bail!("Only 3D volumes are supported (got {:?})", dynamic.shape()); }
        let data = dynamic.into_dimensionality::<Ix3>()?;
        if data.shape().contains(&0) { bail!("NIfTI volume has a zero-sized axis"); }
        Ok(Self { data, spacing })
    }

    pub fn dims(&self) -> [usize; 3] {
        let sh = self.data.shape();
        [sh[0], sh[1], sh[2]]
    }
    pub fn depth(&self, axis: Axis) -> usize { self.dims()[axis.index()] }
    pub fn plane(&self, axis: Axis) -> (usize, usize, f64) {
        let dims = self.dims();
        let (a, b) = axis.plane_axes();
        let aspect = (dims[a] as f64 * self.spacing[a] as f64)
            / (dims[b] as f64 * self.spacing[b] as f64);
        (dims[a], dims[b], aspect)
    }
    #[inline]
    pub fn sample(&self, axis: Axis, slice: usize, h: usize, v: usize) -> f32 {
        match axis {
            Axis::X => self.data[[slice, h, v]],
            Axis::Y => self.data[[h, slice, v]],
            Axis::Z => self.data[[h, v, slice]],
        }
    }

    /// Exact finite minimum/maximum, including zero-valued background.
    /// This is computed only once at startup, not during each redraw.
    pub fn data_range(&self) -> (f32, f32) {
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for &v in self.data.iter() {
            if v.is_finite() {
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
        if lo.is_infinite() { (0.0, 0.0) } else { (lo, hi) }
    }

    /// Deterministic subsample of finite, non-zero voxels for contrast limits.
    /// This avoids sorting or copying millions of voxels.
    pub fn auto_window(&self) -> (f32, f32) {
        let stride = (self.data.len() / 120_000).max(1);
        let mut samples: Vec<f32> = self.data.iter().step_by(stride).copied()
            .filter(|v| v.is_finite() && *v != 0.0).collect();
        if samples.is_empty() { return (0.0, 1.0); }
        samples.sort_unstable_by(f32::total_cmp);
        let lo = samples[(samples.len() - 1) / 100];
        let hi = samples[((samples.len() - 1) as f64 * 0.995) as usize];
        if lo < hi { (lo, hi) }
        else { (lo - 0.5, lo + 0.5) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::Array3;

    fn synthetic() -> Volume {
        Volume { data: Array3::from_shape_fn((3,4,5), |(x,y,z)| (x*100+y*10+z) as f32), spacing: [1.,2.,3.] }
    }
    #[test]
    fn axis_sampling() {
        let v = synthetic();
        assert_eq!(v.sample(Axis::X, 2, 1, 4), 214.);
        assert_eq!(v.sample(Axis::Y, 1, 2, 4), 214.);
        assert_eq!(v.sample(Axis::Z, 4, 2, 1), 214.);
        assert_eq!(v.plane(Axis::X).0, 4);
    }
    #[test]
    fn data_range_includes_zeros_and_ignores_nan() {
        let v = Volume {
            data: Array3::from_shape_vec((2,2,1), vec![0., 10., f32::NAN, -1.]).unwrap(),
            spacing: [1.; 3],
        };
        assert_eq!(v.data_range(), (-1., 10.));
    }
    #[test]
    fn window_ignores_zeros_and_nonfinite() {
        let v = Volume { data: Array3::from_shape_vec((2,2,2), vec![0., 0., 10., 20., 30., f32::NAN, 40., 100.]).unwrap(), spacing: [1.;3] };
        let (lo, hi) = v.auto_window();
        assert_eq!(lo, 10.);
        assert!(hi >= 40.);
    }
}
