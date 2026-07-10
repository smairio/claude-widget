//! Pure visual math for the card: the Claude Code effort palette (exact RGBs extracted
//! from the CC 2.1.205 binary — see spike #3), the effort → backdrop mapping, and the
//! phase functions for the working wave and the max-effort rainbow field.
//!
//! No egui here: the app paints, the core decides what/when. Time is injected (`t_ms`)
//! so every animation frame is a pure function — and testable.

use crate::EffortLevel;

/// Plain (r, g, b) — the app converts to its own color type.
pub type Rgb = (u8, u8, u8);

/// The spark mascot's coral.
pub const SPARK_CORAL: Rgb = (217, 119, 87);

pub const EFFORT_AMBER: Rgb = (255, 193, 7);
pub const EFFORT_GREEN: Rgb = (78, 186, 101);
pub const EFFORT_PERIWINKLE: Rgb = (177, 185, 249);
pub const WAVE_PURPLE_DIM: Rgb = (62, 22, 118);
pub const WAVE_PURPLE_BRIGHT: Rgb = (140, 80, 240);

/// The max-effort rainbow, in wheel order.
pub const RAINBOW: [Rgb; 7] = [
    (235, 95, 87),
    (245, 139, 87),
    (250, 195, 95),
    (145, 200, 130),
    (130, 170, 220),
    (155, 130, 200),
    (200, 130, 180),
];

/// xhigh shimmer period (sinusoidal, from the CC theme).
pub const SHIMMER_PERIOD_MS: u64 = 2400;
/// Max-effort rainbow cycle period.
pub const RAINBOW_PERIOD_MS: u64 = 6000;
/// Working-wave ring period (one ring's birth-to-edge travel time).
pub const WAVE_PERIOD_MS: u64 = 2400;

/// What the card paints behind its content for an effort level at time `t_ms`.
#[derive(Debug, Clone, PartialEq)]
pub enum Backdrop {
    /// Flat dark background — no effort known.
    Plain,
    /// A tint blended over the dark background. Static 18% for low/medium/high; the
    /// xhigh shimmer is also expressed as a Tint whose color/blend oscillate with time.
    Tint { color: Rgb, blend: f32 },
    /// Max effort: the animated rainbow-pixel field; `phase` advances 0→1 per cycle.
    RainbowPixels { phase: f32 },
}

/// Whether the purple wave should radiate. Post-#7 refinement (issue #13): the wave is
/// the TOP TIER's working signature — xhigh only (ultracode rides along; Claude Code
/// reports it as xhigh) — not a general working indicator.
pub fn wave_active(effort: Option<EffortLevel>, any_working: bool) -> bool {
    effort == Some(EffortLevel::XHigh) && any_working
}

/// Map an effort level to its backdrop at `t_ms`.
pub fn effort_backdrop(effort: Option<EffortLevel>, t_ms: u64) -> Backdrop {
    match effort {
        None => Backdrop::Plain,
        Some(EffortLevel::Low) => Backdrop::Tint { color: EFFORT_AMBER, blend: 0.18 },
        Some(EffortLevel::Medium) => Backdrop::Tint { color: EFFORT_GREEN, blend: 0.18 },
        Some(EffortLevel::High) => Backdrop::Tint { color: EFFORT_PERIWINKLE, blend: 0.18 },
        Some(EffortLevel::XHigh) => {
            // Sinusoidal shimmer between the two purples; blend rides 30-35% with it.
            let phase = (t_ms % SHIMMER_PERIOD_MS) as f32 / SHIMMER_PERIOD_MS as f32;
            let s = 0.5 + 0.5 * (phase * std::f32::consts::TAU).sin();
            Backdrop::Tint {
                color: lerp_rgb(WAVE_PURPLE_DIM, WAVE_PURPLE_BRIGHT, s),
                blend: 0.30 + 0.05 * s,
            }
        }
        Some(EffortLevel::Max) => Backdrop::RainbowPixels {
            phase: (t_ms % RAINBOW_PERIOD_MS) as f32 / RAINBOW_PERIOD_MS as f32,
        },
    }
}

/// Linear interpolation between two colors, `t` clamped to [0, 1].
pub fn lerp_rgb(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (ch(a.0, b.0), ch(a.1, b.1), ch(a.2, b.2))
}

/// A smooth color from the rainbow wheel. `phase` is the cycle position (any f32, wraps
/// at 1); `offset` shifts spatially so neighboring pixels differ.
pub fn rainbow_color(phase: f32, offset: f32) -> Rgb {
    let pos = (phase + offset).rem_euclid(1.0) * RAINBOW.len() as f32;
    let i = (pos.floor() as usize) % RAINBOW.len();
    let frac = pos - pos.floor();
    lerp_rgb(RAINBOW[i], RAINBOW[(i + 1) % RAINBOW.len()], frac)
}

/// One ring of the working wave. Rings `k` of `n` are evenly staggered; each is born at
/// the mascot and travels outward over [`WAVE_PERIOD_MS`]. Returns `(progress, alpha)`:
/// `progress` in [0, 1) is how far out the ring is (0 = at the mascot, 1 = fully out),
/// `alpha` in [0, 0.35] fades as the ring travels — successive purple rings with
/// transparency between them, emanating from the mascot.
pub fn wave_ring(k: usize, n: usize, t_ms: u64) -> (f32, f32) {
    let time = (t_ms % WAVE_PERIOD_MS) as f32 / WAVE_PERIOD_MS as f32;
    let progress = (time + k as f32 / n.max(1) as f32).rem_euclid(1.0);
    (progress, 0.35 * (1.0 - progress))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EffortLevel::*;

    #[test]
    fn effort_levels_parse_from_payload_strings() {
        assert_eq!(EffortLevel::from_level("low"), Some(Low));
        assert_eq!(EffortLevel::from_level("medium"), Some(Medium));
        assert_eq!(EffortLevel::from_level("high"), Some(High));
        assert_eq!(EffortLevel::from_level("xhigh"), Some(XHigh));
        assert_eq!(EffortLevel::from_level("max"), Some(Max));
        // The CLI gives ultracode the xhigh treatment.
        assert_eq!(EffortLevel::from_level("ultracode"), Some(XHigh));
        assert_eq!(EffortLevel::from_level("MAX"), Some(Max), "case-insensitive");
        assert_eq!(EffortLevel::from_level("hyper"), None);
    }

    #[test]
    fn wave_radiates_only_at_xhigh_while_working() {
        assert!(wave_active(Some(XHigh), true));
        // Not while idle, even at xhigh.
        assert!(!wave_active(Some(XHigh), false));
        // Not at any other tier, nor with unknown effort.
        assert!(!wave_active(Some(Max), true));
        assert!(!wave_active(Some(High), true));
        assert!(!wave_active(Some(Low), true));
        assert!(!wave_active(None, true));
    }

    #[test]
    fn static_efforts_map_to_18_percent_tints() {
        for (level, color) in [
            (Low, EFFORT_AMBER),
            (Medium, EFFORT_GREEN),
            (High, EFFORT_PERIWINKLE),
        ] {
            let b = effort_backdrop(Some(level), 0);
            assert_eq!(b, Backdrop::Tint { color, blend: 0.18 });
            // Static: time does not change them.
            assert_eq!(effort_backdrop(Some(level), 123_456), b);
        }
        assert_eq!(effort_backdrop(None, 5), Backdrop::Plain);
    }

    #[test]
    fn xhigh_shimmers_between_the_two_purples() {
        let quarter = SHIMMER_PERIOD_MS / 4;
        let (Backdrop::Tint { color: c1, blend: b1 }, Backdrop::Tint { color: c2, blend: b2 }) = (
            effort_backdrop(Some(XHigh), quarter),      // sin peak -> bright
            effort_backdrop(Some(XHigh), 3 * quarter),  // sin trough -> dim
        ) else {
            panic!("xhigh must be a Tint backdrop");
        };
        assert_eq!(c1, WAVE_PURPLE_BRIGHT);
        assert_eq!(c2, WAVE_PURPLE_DIM);
        // Blend oscillates inside the 30-35% band, brightest = strongest.
        assert!((b1 - 0.35).abs() < 1e-4, "peak blend 35%, got {b1}");
        assert!((b2 - 0.30).abs() < 1e-4, "trough blend 30%, got {b2}");
        // Periodic.
        assert_eq!(
            effort_backdrop(Some(XHigh), quarter),
            effort_backdrop(Some(XHigh), quarter + SHIMMER_PERIOD_MS)
        );
    }

    #[test]
    fn max_is_the_rainbow_field() {
        let Backdrop::RainbowPixels { phase } = effort_backdrop(Some(Max), RAINBOW_PERIOD_MS / 2)
        else {
            panic!("max must be RainbowPixels");
        };
        assert!((phase - 0.5).abs() < 1e-4);
        // Wraps each period.
        let Backdrop::RainbowPixels { phase } = effort_backdrop(Some(Max), RAINBOW_PERIOD_MS)
        else {
            panic!()
        };
        assert!(phase < 1e-4);
    }

    #[test]
    fn lerp_rgb_hits_its_endpoints() {
        assert_eq!(lerp_rgb((0, 0, 0), (255, 255, 255), 0.0), (0, 0, 0));
        assert_eq!(lerp_rgb((0, 0, 0), (255, 255, 255), 1.0), (255, 255, 255));
        assert_eq!(lerp_rgb((0, 0, 0), (200, 100, 50), 0.5), (100, 50, 25));
        // Clamped outside [0,1].
        assert_eq!(lerp_rgb((0, 0, 0), (255, 255, 255), 2.0), (255, 255, 255));
        assert_eq!(lerp_rgb((0, 0, 0), (255, 255, 255), -1.0), (0, 0, 0));
    }

    #[test]
    fn rainbow_wheel_wraps_and_shifts() {
        assert_eq!(rainbow_color(0.0, 0.0), RAINBOW[0]);
        // Exactly one wheel step ahead.
        assert_eq!(rainbow_color(0.0, 1.0 / 7.0), RAINBOW[1]);
        // Wraps at 1.
        assert_eq!(rainbow_color(1.0, 0.0), rainbow_color(0.0, 0.0));
        assert_eq!(rainbow_color(0.5, 0.7), rainbow_color(1.5, 0.7));
        // Between two stops it interpolates (differs from both).
        let mid = rainbow_color(0.5 / 7.0, 0.0);
        assert_ne!(mid, RAINBOW[0]);
        assert_ne!(mid, RAINBOW[1]);
    }

    #[test]
    fn wave_rings_travel_outward_staggered_and_fade() {
        let (p0, a0) = wave_ring(0, 3, 0);
        assert_eq!(p0, 0.0, "ring 0 is born at t=0");
        assert!((a0 - 0.35).abs() < 1e-4, "newborn ring at peak alpha");
        // Travels outward with time…
        let (p1, a1) = wave_ring(0, 3, WAVE_PERIOD_MS / 2);
        assert!((p1 - 0.5).abs() < 1e-4);
        assert!(a1 < a0, "alpha fades as the ring travels");
        // …and wraps after a full period.
        let (p2, _) = wave_ring(0, 3, WAVE_PERIOD_MS);
        assert!(p2 < 1e-4);
        // Rings are evenly staggered: purple, gap, purple.
        let (q, _) = wave_ring(1, 3, 0);
        let (r, _) = wave_ring(2, 3, 0);
        assert!((q - 1.0 / 3.0).abs() < 1e-4);
        assert!((r - 2.0 / 3.0).abs() < 1e-4);
        // Alpha stays in bounds everywhere.
        for t in (0..5000).step_by(97) {
            let (p, a) = wave_ring(1, 3, t);
            assert!((0.0..1.0).contains(&p));
            assert!((0.0..=0.35).contains(&a));
        }
    }
}
