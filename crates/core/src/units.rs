//! Blood-glucose display units. Storage is always mg/dL; conversion to a
//! display unit is presentation-only. Three units are supported:
//!   - mg/dL: the stored representation, no conversion.
//!   - mmol/L: divide by 18.0, one decimal.
//!   - Kovatchev risk space ("kovachev"): the symmetrizing transform
//!     f(BG) = 1.509 * (ln(BG)^1.084 - 5.381), which maps mg/dL onto a
//!     scale symmetric about 112.5 mg/dL (f = 0), with hypo below and
//!     hyper above. Two decimals. The inverse recovers mg/dL.

use serde::{Deserialize, Serialize};

/// mg/dL per mmol/L. The canonical BG conversion factor.
pub const MGDL_PER_MMOL: f64 = 18.0;

/// Kovatchev symmetrization scale factor.
const KOV_SCALE: f64 = 1.509;
/// Kovatchev log exponent.
const KOV_EXP: f64 = 1.084;
/// Kovatchev offset (centres the transform at 112.5 mg/dL).
const KOV_OFFSET: f64 = 5.381;

/// Display unit for blood glucose. Storage is fixed mg/dL; this only ever
/// governs presentation and the TUI toggle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BgUnit {
    #[default]
    Mgdl,
    Mmol,
    Kovachev,
}

impl BgUnit {
    /// Convert a stored mg/dL value into this display unit.
    #[inline]
    pub fn from_mgdl(self, mgdl: f64) -> f64 {
        match self {
            BgUnit::Mgdl => mgdl,
            BgUnit::Mmol => mgdl / MGDL_PER_MMOL,
            BgUnit::Kovachev => {
                if mgdl <= 0.0 {
                    0.0
                } else {
                    KOV_SCALE * (mgdl.ln().powf(KOV_EXP) - KOV_OFFSET)
                }
            }
        }
    }

    /// Convert a value expressed in this display unit back to stored mg/dL.
    #[inline]
    pub fn to_mgdl(self, value: f64) -> f64 {
        match self {
            BgUnit::Mgdl => value,
            BgUnit::Mmol => value * MGDL_PER_MMOL,
            BgUnit::Kovachev => (value / KOV_SCALE + KOV_OFFSET).powf(1.0 / KOV_EXP).exp(),
        }
    }

    /// Short label used in the header/footer.
    pub fn label(self) -> &'static str {
        match self {
            BgUnit::Mgdl => "mg/dL",
            BgUnit::Mmol => "mmol/L",
            BgUnit::Kovachev => "kov",
        }
    }

    /// Decimals to render for this unit.
    pub fn decimals(self) -> usize {
        match self {
            BgUnit::Mgdl => 0,
            BgUnit::Mmol => 1,
            BgUnit::Kovachev => 2,
        }
    }

    /// Format a stored mg/dL value for display, with unit-appropriate decimals.
    pub fn format(self, mgdl: f64) -> String {
        format!("{:.*}", self.decimals(), self.from_mgdl(mgdl))
    }

    /// Cycle to the other unit (TUI `u` key).
    pub fn toggled(self) -> Self {
        match self {
            BgUnit::Mgdl => BgUnit::Mmol,
            BgUnit::Mmol => BgUnit::Kovachev,
            BgUnit::Kovachev => BgUnit::Mgdl,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kovachev_symmetric_about_112_5() {
        assert!((BgUnit::Kovachev.from_mgdl(112.5)).abs() < 0.01);
        assert!((BgUnit::Kovachev.from_mgdl(70.0) - (-0.88)).abs() < 0.02);
        assert!((BgUnit::Kovachev.from_mgdl(180.0) - 0.88).abs() < 0.02);
    }

    #[test]
    fn kovachev_roundtrips() {
        for &bg in &[54.0, 70.0, 112.5, 180.0, 300.0] {
            let k = BgUnit::Kovachev.from_mgdl(bg);
            assert!((BgUnit::Kovachev.to_mgdl(k) - bg).abs() < 1e-6);
        }
    }

    #[test]
    fn kovachev_guards_nonpositive() {
        assert_eq!(BgUnit::Kovachev.from_mgdl(0.0), 0.0);
        assert_eq!(BgUnit::Kovachev.from_mgdl(-5.0), 0.0);
    }

    #[test]
    fn toggled_cycles_three() {
        assert_eq!(BgUnit::Mgdl.toggled(), BgUnit::Mmol);
        assert_eq!(BgUnit::Mmol.toggled(), BgUnit::Kovachev);
        assert_eq!(BgUnit::Kovachev.toggled(), BgUnit::Mgdl);
    }
}
