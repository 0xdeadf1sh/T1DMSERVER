//! Blood-glucose display units. Storage is always mg/dL; conversion to a
//! display unit is presentation-only. Three units are supported:
//!   - mg/dL: the stored representation, no conversion.
//!   - mmol/L: divide by 18.0182 (the SI molar-mass conversion for glucose),
//!     one decimal.
//!   - Kovatchev risk space ("kovachev"): the symmetrizing transform
//!     f(BG) = 1.509 * (ln(BG)^1.084 - 5.381), which maps mg/dL onto a
//!     scale symmetric about 112.5 mg/dL (f = 0), with hypo below and
//!     hyper above. Two decimals. The inverse recovers mg/dL.

use serde::{Deserialize, Serialize};

/// mg/dL per mmol/L. The canonical BG conversion factor.
pub const MGDL_PER_MMOL: f64 = 18.0182;

/// Kovatchev symmetrization scale factor.
const KOV_SCALE: f64 = 1.509;
/// Kovatchev log exponent.
const KOV_EXP: f64 = 1.084;
/// Kovatchev offset (centres the transform at 112.5 mg/dL).
const KOV_OFFSET: f64 = 5.381;
/// Physical BG bounds of the Kovatchev domain. `from_mgdl` clamps its input to
/// these and `to_mgdl` its output; a garbage BG must read as maximal hypo risk,
/// never as the risk-neutral f = 0 (which would say "perfect euglycemia").
const KOV_BG_MIN: f64 = 20.0;
const KOV_BG_MAX: f64 = 600.0;

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
                let g = if mgdl.is_nan() {
                    KOV_BG_MIN
                } else {
                    mgdl.clamp(KOV_BG_MIN, KOV_BG_MAX)
                };
                KOV_SCALE * (g.ln().powf(KOV_EXP) - KOV_OFFSET)
            }
        }
    }

    /// Convert a value expressed in this display unit back to stored mg/dL.
    #[inline]
    pub fn to_mgdl(self, value: f64) -> f64 {
        match self {
            BgUnit::Mgdl => value,
            BgUnit::Mmol => value * MGDL_PER_MMOL,
            BgUnit::Kovachev => {
                let lo = BgUnit::Kovachev.from_mgdl(KOV_BG_MIN);
                let hi = BgUnit::Kovachev.from_mgdl(KOV_BG_MAX);
                let r = if value.is_nan() || value == f64::NEG_INFINITY {
                    lo
                } else if value == f64::INFINITY {
                    hi
                } else {
                    value
                };
                // The clamp keeps the power base >= 0, so no NaN and no exp overflow.
                let r = r.clamp(lo, hi);
                (r / KOV_SCALE + KOV_OFFSET)
                    .powf(1.0 / KOV_EXP)
                    .exp()
                    .clamp(KOV_BG_MIN, KOV_BG_MAX)
            }
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

    /// Format a mg/dL *dispersion* (a standard deviation, an interval width)
    /// for display. A spread has no zero point, so only a scale-preserving
    /// conversion is meaningful: pushing it through [`from_mgdl`](Self::from_mgdl)
    /// would apply the Kovatchev level transform, which is centred on
    /// 112.5 mg/dL and would render a typical SD as a negative number. The risk
    /// scale has no dispersion image at all, so it falls back to mg/dL.
    pub fn format_spread(self, mgdl: f64) -> String {
        match self {
            BgUnit::Mgdl | BgUnit::Kovachev => format!("{:.*}", BgUnit::Mgdl.decimals(), mgdl),
            BgUnit::Mmol => format!("{:.*}", self.decimals(), mgdl / MGDL_PER_MMOL),
        }
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
    fn kovachev_clamps_to_the_physical_domain() {
        // Out-of-domain BG lands on the RAIL, never on the risk-neutral f = 0
        // (which would read as ~112.5 mg/dL, perfect euglycemia).
        let lo = BgUnit::Kovachev.from_mgdl(20.0);
        let hi = BgUnit::Kovachev.from_mgdl(600.0);
        assert!((lo - (-3.1633)).abs() < 1e-3, "f(20) = {lo}");
        assert!((hi - 3.1619).abs() < 1e-3, "f(600) = {hi}");
        // The domain is the one the transform was built symmetric over:
        // f(20) and f(600) are equal and opposite to within a thousandth.
        assert!((lo + hi).abs() < 1e-2, "f(20) + f(600) = {}", lo + hi);
        assert_eq!(BgUnit::Kovachev.from_mgdl(0.0), lo);
        assert_eq!(BgUnit::Kovachev.from_mgdl(-5.0), lo);
        assert_eq!(BgUnit::Kovachev.from_mgdl(f64::NAN), lo);
        assert_eq!(BgUnit::Kovachev.from_mgdl(700.0), hi);
    }

    #[test]
    fn kovachev_inverse_is_guarded() {
        let lo = BgUnit::Kovachev.from_mgdl(20.0);
        let hi = BgUnit::Kovachev.from_mgdl(600.0);
        assert!((BgUnit::Kovachev.to_mgdl(lo - 10.0) - 20.0).abs() < 1e-6);
        assert!((BgUnit::Kovachev.to_mgdl(hi + 10.0) - 600.0).abs() < 1e-6);
        assert!((BgUnit::Kovachev.to_mgdl(f64::NAN) - 20.0).abs() < 1e-6);
        assert!((BgUnit::Kovachev.to_mgdl(f64::NEG_INFINITY) - 20.0).abs() < 1e-6);
        assert!((BgUnit::Kovachev.to_mgdl(f64::INFINITY) - 600.0).abs() < 1e-6);
    }

    #[test]
    fn mmol_uses_the_si_molar_mass_factor() {
        // 18.0182, not 18.0: the two straddle the one-decimal rounding boundary
        // for roughly every ninth integer mg/dL (100 → 5.5, not 5.6).
        assert_eq!(BgUnit::Mmol.format(100.0), "5.5");
        assert_eq!(BgUnit::Mmol.format(180.0), "10.0");
        assert!((BgUnit::Mmol.from_mgdl(90.091) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn format_spread_is_scale_preserving() {
        assert_eq!(BgUnit::Mgdl.format_spread(50.6), "51");
        assert_eq!(BgUnit::Mmol.format_spread(50.6), "2.8");
        assert_eq!(BgUnit::Kovachev.format_spread(50.6), "51");
    }

    #[test]
    fn format_spread_never_negates_a_positive_spread() {
        for &sd in &[5.0, 20.0, 50.6, 90.0] {
            for unit in [BgUnit::Mgdl, BgUnit::Mmol, BgUnit::Kovachev] {
                assert!(!unit.format_spread(sd).starts_with('-'));
            }
        }
        assert!(BgUnit::Kovachev.format(50.6).starts_with('-'));
    }

    #[test]
    fn format_spread_zero_stays_zero() {
        assert_eq!(BgUnit::Mgdl.format_spread(0.0), "0");
        assert_eq!(BgUnit::Mmol.format_spread(0.0), "0.0");
        assert_eq!(BgUnit::Kovachev.format_spread(0.0), "0");
    }

    #[test]
    fn toggled_cycles_three() {
        assert_eq!(BgUnit::Mgdl.toggled(), BgUnit::Mmol);
        assert_eq!(BgUnit::Mmol.toggled(), BgUnit::Kovachev);
        assert_eq!(BgUnit::Kovachev.toggled(), BgUnit::Mgdl);
    }
}
