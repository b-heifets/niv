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
        match self {
            Self::X => 0,
            Self::Y => 1,
            Self::Z => 2,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::X => "X",
            Self::Y => "Y",
            Self::Z => "Z",
        }
    }
    pub fn xyz(self, slice: usize, h: usize, v: usize) -> [usize; 3] {
        match self {
            Self::X => [slice, h, v],
            Self::Y => [h, slice, v],
            Self::Z => [h, v, slice],
        }
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
    /// S-form transform when supplied by NIfTI, used to reject misregistered overlays.
    pub sform: Option<[[f32; 4]; 3]>,
    /// Datatype stored in the original NIfTI header before f32 conversion.
    pub datatype: i16,
}

impl Volume {
    pub fn load(path: &Path) -> Result<Self> {
        let object = ReaderOptions::new()
            .read_file(path)
            .with_context(|| format!("Failed to read NIfTI file: {}", path.display()))?;
        // Reject 4D/5D acquisitions before converting all voxels to f32.
        let dims = object.header().dim;
        let rank = dims[0] as usize;
        if !(3..=7).contains(&rank) || (4..=rank).any(|axis| dims[axis] != 1) {
            bail!(
                "Only 3D NIfTI volumes are supported (header dimensions {:?})",
                dims
            );
        }
        let header = object.header();
        let datatype = header.datatype;
        let sform = if header.sform_code != 0 {
            Some([header.srow_x, header.srow_y, header.srow_z])
        } else {
            None
        };
        let mut spacing = [1f32; 3];
        for (axis, s) in spacing.iter_mut().enumerate() {
            let value = object.header().pixdim[axis + 1].abs();
            if value.is_finite() && value > 0.0 {
                *s = value;
            }
        }
        // nifti's into_ndarray applies scl_slope/scl_inter, then casts to f32.
        // The resulting ndarray usually retains NIfTI's Fortran-order layout.
        let mut dynamic = object
            .into_volume()
            .into_ndarray::<f32>()
            .context("Failed to decode voxel data")?;
        while dynamic.ndim() > 3 {
            let last = dynamic.ndim() - 1;
            if dynamic.shape()[last] != 1 {
                bail!(
                    "Expected a 3D volume or singleton higher dimensions; got {:?}",
                    dynamic.shape()
                );
            }
            dynamic = dynamic.index_axis_move(NdAxis(last), 0);
        }
        if dynamic.ndim() != 3 {
            bail!("Only 3D volumes are supported (got {:?})", dynamic.shape());
        }
        let data = dynamic.into_dimensionality::<Ix3>()?;
        if data.shape().contains(&0) {
            bail!("NIfTI volume has a zero-sized axis");
        }
        Ok(Self {
            data,
            spacing,
            sform,
            datatype,
        })
    }

    /// Only voxel-to-voxel overlays are supported. We intentionally reject
    /// mismatched grids instead of silently displaying a misregistered atlas.
    pub fn validate_grid(&self, other: &Self) -> Result<()> {
        if self.dims() != other.dims() {
            bail!(
                "NIfTI grid dimensions differ: {:?} vs {:?}; resample before layering",
                self.dims(),
                other.dims()
            );
        }
        if self
            .spacing
            .iter()
            .zip(other.spacing.iter())
            .any(|(a, b)| (a - b).abs() > 1e-4 * a.max(*b).max(1.0))
        {
            bail!(
                "NIfTI voxel spacing differs: {:?} vs {:?}; resample before layering",
                self.spacing,
                other.spacing
            );
        }
        match (&self.sform, &other.sform) {
            (Some(a), Some(b)) => {
                if (0..3).any(|r| (0..4).any(|c| (a[r][c] - b[r][c]).abs() > 1e-3)) {
                    bail!("NIfTI sform transforms differ; resample into the same grid before layering");
                }
            },
            (None, None) => {},
            _ => bail!("One NIfTI has an sform transform but the other does not; align the headers/grids before layering"),
        }
        Ok(())
    }

    pub fn dims(&self) -> [usize; 3] {
        let sh = self.data.shape();
        [sh[0], sh[1], sh[2]]
    }
    pub fn depth(&self, axis: Axis) -> usize {
        self.dims()[axis.index()]
    }
    pub fn plane(&self, axis: Axis) -> (usize, usize, f64) {
        let dims = self.dims();
        let (a, b) = axis.plane_axes();
        let aspect =
            (dims[a] as f64 * self.spacing[a] as f64) / (dims[b] as f64 * self.spacing[b] as f64);
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
        if lo.is_infinite() {
            (0.0, 0.0)
        } else {
            (lo, hi)
        }
    }

    /// Exact finite data range, with a nonzero window for constant volumes.
    /// Values <= display minimum are transparent, by design.
    #[cfg(test)]
    pub fn auto_window(&self) -> (f32, f32) {
        let (lo, hi) = self.data_range();
        if lo < hi {
            (lo, hi)
        } else {
            (lo - 0.5, hi + 0.5)
        }
    }

    pub fn datatype_name(&self) -> &'static str {
        match self.datatype {
            2 => "uint8",
            4 => "int16",
            8 => "int32",
            16 => "float32",
            64 => "float64",
            128 => "RGB24",
            256 => "int8",
            512 => "uint16",
            768 => "uint32",
            1024 => "int64",
            1280 => "uint64",
            1536 => "float128",
            1792 => "complex128",
            _ => "other",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::Array3;

    fn synthetic() -> Volume {
        Volume {
            data: Array3::from_shape_fn((3, 4, 5), |(x, y, z)| (x * 100 + y * 10 + z) as f32),
            spacing: [1., 2., 3.],
            sform: None,
            datatype: 16,
        }
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
    fn rejects_misaligned_overlay_grid() {
        let a = synthetic();
        let mut b = synthetic();
        assert!(a.validate_grid(&b).is_ok());
        b.spacing[1] = 3.;
        assert!(a.validate_grid(&b).is_err());
        b.spacing = a.spacing;
        b.sform = Some([[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]]);
        assert!(a.validate_grid(&b).is_err());
    }
    #[test]
    fn data_range_includes_zeros_and_ignores_nan() {
        let v = Volume {
            data: Array3::from_shape_vec((2, 2, 1), vec![0., 10., f32::NAN, -1.]).unwrap(),
            spacing: [1.; 3],
            sform: None,
            datatype: 16,
        };
        assert_eq!(v.data_range(), (-1., 10.));
    }
    #[test]
    fn window_defaults_to_full_finite_range() {
        let v = Volume {
            data: Array3::from_shape_vec(
                (2, 2, 2),
                vec![0., 0., 10., 20., 30., f32::NAN, 40., 100.],
            )
            .unwrap(),
            spacing: [1.; 3],
            sform: None,
            datatype: 16,
        };
        let (lo, hi) = v.auto_window();
        assert_eq!((lo, hi), (0., 100.));
    }
}
