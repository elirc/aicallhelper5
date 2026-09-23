//! Window geometry — pure functions over physical pixels (spec §13).
//!
//! Coordinates are virtual-screen physical pixels and may be negative (a
//! monitor left of / above the primary). `Bounds` is the window's OUTER
//! position with its INNER size (what Tauri's `set_position` / `set_size`
//! take); the ~16 px frame difference is irrelevant to these rules.

use callcore_contract::{Bounds, LayoutMode};

/// A rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
    fn right(&self) -> i64 {
        self.x as i64 + self.width as i64
    }
    fn bottom(&self) -> i64 {
        self.y as i64 + self.height as i64
    }
    /// Size of the intersection, as (width, height); zero when disjoint.
    pub fn intersection(&self, other: &Rect) -> (u32, u32) {
        let l = (self.x as i64).max(other.x as i64);
        let t = (self.y as i64).max(other.y as i64);
        let r = self.right().min(other.right());
        let b = self.bottom().min(other.bottom());
        if r <= l || b <= t {
            (0, 0)
        } else {
            ((r - l) as u32, (b - t) as u32)
        }
    }
}

impl From<Bounds> for Rect {
    fn from(b: Bounds) -> Self {
        Rect {
            x: b.x,
            y: b.y,
            width: b.width,
            height: b.height,
        }
    }
}

/// A connected display: its work area (excludes the taskbar) in physical
/// pixels and its DPI scale (1.0 = 100 %, 1.25, 1.5, …).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Monitor {
    pub work_area: Rect,
    pub scale: f64,
}

/// A size in LOGICAL pixels (scaled by a monitor's DPI to get physical).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogicalSize {
    pub width: u32,
    pub height: u32,
}

impl LogicalSize {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
    pub fn to_physical(self, scale: f64) -> (u32, u32) {
        let s = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        (
            (self.width as f64 * s).round() as u32,
            (self.height as f64 * s).round() as u32,
        )
    }
}

pub const FULL_MIN: LogicalSize = LogicalSize::new(380, 520);
pub const FULL_DEFAULT: LogicalSize = LogicalSize::new(440, 640);
pub const PROMPTER_MIN: LogicalSize = LogicalSize::new(380, 160);
/// Clamped to the work-area width by `restore`.
pub const PROMPTER_DEFAULT: LogicalSize = LogicalSize::new(900, 200);

/// `(min, default)` logical sizes for a layout.
pub fn layout_sizes(layout: LayoutMode) -> (LogicalSize, LogicalSize) {
    match layout {
        LayoutMode::Full => (FULL_MIN, FULL_DEFAULT),
        LayoutMode::Prompter => (PROMPTER_MIN, PROMPTER_DEFAULT),
    }
}

/// Height of the title-bar band that must stay reachable (logical px).
pub const TITLE_STRIP: u32 = 32;
/// Minimum visible width of that band on some monitor (physical px).
pub const MIN_VISIBLE: u32 = 40;
/// Minimum visible height of that band (physical px) — enough to grab it.
pub const MIN_VISIBLE_STRIP_HEIGHT: u32 = 16;

/// Top of the work area, horizontally centred. A window wider than the work
/// area starts at its left edge (so the title bar stays reachable).
pub fn dock_top_centre(monitor: &Monitor, width: u32, height: u32) -> Bounds {
    let wa = monitor.work_area;
    let x = if width >= wa.width {
        wa.x
    } else {
        (wa.x as i64 + ((wa.width - width) / 2) as i64) as i32
    };
    Bounds {
        x,
        y: wa.y,
        width,
        height,
    }
}

/// True when a reachable title-bar strip of `b` (≥ 40 px wide, ≥ 16 px tall)
/// lies on SOME monitor's work area.
pub fn is_reachable(b: &Bounds, monitors: &[Monitor]) -> bool {
    monitors.iter().any(|m| {
        let strip_h = ((TITLE_STRIP as f64) * sane_scale(m.scale)).round() as u32;
        let strip = Rect::new(b.x, b.y, b.width, strip_h.min(b.height.max(1)));
        let (w, h) = strip.intersection(&m.work_area);
        w >= MIN_VISIBLE && h >= MIN_VISIBLE_STRIP_HEIGHT.min(strip.height)
    })
}

fn sane_scale(s: f64) -> f64 {
    if s.is_finite() && s > 0.0 {
        s
    } else {
        1.0
    }
}

/// Clamp a physical size to fit `wa`, then to at least `min` (min wins on a
/// work area smaller than the minimum — the OS enforces min size anyway).
fn fit(width: u32, height: u32, wa: &Rect, min: (u32, u32)) -> (u32, u32) {
    let w = width.min(wa.width.max(1)).max(min.0);
    let h = height.min(wa.height.max(1)).max(min.1);
    (w, h)
}

/// Decide where the window goes at startup / layout switch.
///
/// * `saved` is reused (size clamped to ≥ min) only when [`is_reachable`].
/// * Otherwise dock top-centre of `current` (else the first monitor, which
///   the caller puts first as the primary) at the saved size, or the default
///   size, clamped to fit the work area and ≥ min.
/// * With no monitor info at all, a usable size at (0, 0).
pub fn restore(
    saved: Option<Bounds>,
    monitors: &[Monitor],
    current: Option<&Monitor>,
    default_size: LogicalSize,
    min_size: LogicalSize,
) -> Bounds {
    if let Some(b) = saved {
        if b.width > 0 && b.height > 0 && is_reachable(&b, monitors) {
            // Clamp against the monitor showing most of the title strip.
            let host = monitors
                .iter()
                .max_by_key(|m| {
                    let (w, h) = Rect::from(b).intersection(&m.work_area);
                    w as u64 * h as u64
                })
                .expect("reachable implies a monitor");
            let min = min_size.to_physical(host.scale);
            let wa = host.work_area;
            let width = b.width.max(min.0);
            let height = b.height.max(min.1);
            // Only shrink when the window is larger than the whole work area.
            let width = if width > wa.width {
                wa.width.max(min.0)
            } else {
                width
            };
            let height = if height > wa.height {
                wa.height.max(min.1)
            } else {
                height
            };
            return Bounds {
                x: b.x,
                y: b.y,
                width,
                height,
            };
        }
    }

    let target = current.or_else(|| monitors.first());
    match target {
        Some(m) => {
            let scale = sane_scale(m.scale);
            let (w, h) = match saved {
                Some(b) if b.width > 0 && b.height > 0 => (b.width, b.height),
                _ => default_size.to_physical(scale),
            };
            let (w, h) = fit(w, h, &m.work_area, min_size.to_physical(scale));
            dock_top_centre(m, w, h)
        }
        None => {
            let (dw, dh) = default_size.to_physical(1.0);
            let (mw, mh) = min_size.to_physical(1.0);
            let (w, h) = match saved {
                Some(b) if b.width > 0 && b.height > 0 => (b.width, b.height),
                _ => (dw, dh),
            };
            Bounds {
                x: 0,
                y: 0,
                width: w.max(mw),
                height: h.max(mh),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(x: i32, y: i32, w: u32, h: u32, scale: f64) -> Monitor {
        Monitor {
            work_area: Rect::new(x, y, w, h),
            scale,
        }
    }
    fn b(x: i32, y: i32, w: u32, h: u32) -> Bounds {
        Bounds {
            x,
            y,
            width: w,
            height: h,
        }
    }

    /// Primary 1920×1040 work area at (0,0); left screen 1920×1040 at x=-1920.
    fn dual_negative() -> Vec<Monitor> {
        vec![mon(0, 0, 1920, 1040, 1.0), mon(-1920, 0, 1920, 1040, 1.0)]
    }

    #[test]
    fn dock_centres_on_the_work_area_top() {
        let m = mon(0, 0, 1920, 1040, 1.0);
        assert_eq!(dock_top_centre(&m, 440, 640), b(740, 0, 440, 640));
        // Top taskbar: work area starts lower.
        let m = mon(0, 48, 1920, 1032, 1.0);
        assert_eq!(dock_top_centre(&m, 400, 300).y, 48);
    }

    #[test]
    fn dock_on_negative_monitor() {
        let m = mon(-1920, -200, 1920, 1080, 1.0);
        assert_eq!(dock_top_centre(&m, 920, 200), b(-1420, -200, 920, 200));
    }

    #[test]
    fn dock_wider_than_work_area_starts_at_left_edge() {
        let m = mon(100, 0, 800, 600, 1.0);
        assert_eq!(dock_top_centre(&m, 900, 200).x, 100);
    }

    #[test]
    fn saved_position_on_left_negative_monitor_is_reused() {
        let mons = dual_negative();
        let saved = b(-1500, 100, 440, 640);
        assert_eq!(
            restore(Some(saved), &mons, None, FULL_DEFAULT, FULL_MIN),
            saved
        );
    }

    #[test]
    fn saved_position_from_unplugged_monitor_docks_on_current() {
        // Saved on a left monitor that is now gone.
        let mons = vec![mon(0, 0, 1920, 1040, 1.0)];
        let saved = b(-1500, 100, 500, 700);
        let r = restore(Some(saved), &mons, Some(&mons[0]), FULL_DEFAULT, FULL_MIN);
        assert_eq!(r, b(710, 0, 500, 700), "docked, saved size kept");
    }

    #[test]
    fn title_bar_off_screen_top_is_not_reachable() {
        let mons = vec![mon(0, 0, 1920, 1040, 1.0)];
        // Body visible, but the title strip is above the work area.
        let saved = b(200, -100, 440, 640);
        assert!(!is_reachable(&saved, &mons));
        let r = restore(Some(saved), &mons, None, FULL_DEFAULT, FULL_MIN);
        assert_eq!(r.y, 0);
    }

    #[test]
    fn only_a_sliver_visible_is_not_reachable() {
        let mons = vec![mon(0, 0, 1920, 1040, 1.0)];
        // 30 px of width on-screen: below the 40 px rule.
        assert!(!is_reachable(&b(1890, 100, 440, 640), &mons));
        // 40 px: enough.
        assert!(is_reachable(&b(1880, 100, 440, 640), &mons));
    }

    #[test]
    fn straddling_two_monitors_is_reachable() {
        let mons = dual_negative();
        assert!(is_reachable(&b(-200, 10, 440, 640), &mons));
    }

    #[test]
    fn stacked_monitors() {
        // Secondary stacked ABOVE the primary.
        let mons = vec![mon(0, 0, 1920, 1040, 1.0), mon(0, -1080, 1920, 1040, 1.0)];
        let saved = b(300, -900, 440, 640);
        assert_eq!(
            restore(Some(saved), &mons, None, FULL_DEFAULT, FULL_MIN),
            saved
        );
        // In the 40 px gap between the stacked work areas (taskbar band).
        let gap = b(300, -40, 440, 640);
        // Title strip rows -40..-8 hit neither work area (upper ends at -40).
        assert!(!is_reachable(&gap, &mons));
    }

    #[test]
    fn mixed_dpi_default_size_scales_with_the_target_monitor() {
        let m100 = mon(0, 0, 1920, 1040, 1.0);
        let m125 = mon(1920, 0, 2560, 1400, 1.25);
        let m150 = mon(-2880, 0, 2880, 1680, 1.5);
        let mons = vec![m100, m125, m150];
        assert_eq!(
            restore(None, &mons, Some(&m100), FULL_DEFAULT, FULL_MIN),
            b(740, 0, 440, 640)
        );
        assert_eq!(
            restore(None, &mons, Some(&m125), FULL_DEFAULT, FULL_MIN),
            b(1920 + (2560 - 550) / 2, 0, 550, 800)
        );
        assert_eq!(
            restore(None, &mons, Some(&m150), FULL_DEFAULT, FULL_MIN),
            b(-2880 + (2880 - 660) / 2, 0, 660, 960)
        );
        // Title strip scales too: at 150 % a 32 px band is 48 px physical.
        let saved = b(-2000, -20, 660, 960);
        assert!(is_reachable(&saved, &mons), "28 visible rows ≥ 16");
    }

    #[test]
    fn saved_size_is_clamped_to_min() {
        let mons = vec![mon(0, 0, 1920, 1040, 1.25)];
        let r = restore(
            Some(b(10, 10, 100, 100)),
            &mons,
            None,
            FULL_DEFAULT,
            FULL_MIN,
        );
        assert_eq!((r.width, r.height), (475, 650));
    }

    #[test]
    fn tiny_work_area_clamps_but_never_below_min() {
        let tiny = mon(0, 0, 800, 500, 1.0);
        let r = restore(None, &[tiny], None, FULL_DEFAULT, FULL_MIN);
        assert_eq!(
            (r.width, r.height),
            (440, 520),
            "height: min wins over the 500 px work area"
        );
        let r = restore(None, &[tiny], None, PROMPTER_DEFAULT, PROMPTER_MIN);
        assert_eq!(r, b(0, 0, 800, 200), "prompter clamped to work-area width");
    }

    #[test]
    fn prompter_default_fits_wide_screen() {
        let m = mon(0, 0, 1920, 1040, 1.0);
        let r = restore(None, &[m], None, PROMPTER_DEFAULT, PROMPTER_MIN);
        assert_eq!(r, b(510, 0, 900, 200));
    }

    #[test]
    fn no_monitors_still_gives_a_usable_size() {
        let r = restore(None, &[], None, FULL_DEFAULT, FULL_MIN);
        assert_eq!(r, b(0, 0, 440, 640));
        let r = restore(
            Some(b(5000, 5000, 10, 10)),
            &[],
            None,
            FULL_DEFAULT,
            FULL_MIN,
        );
        assert_eq!(r, b(0, 0, 380, 520));
    }

    #[test]
    fn first_run_docks_top_centre_of_primary() {
        let mons = dual_negative();
        let r = restore(None, &mons, None, FULL_DEFAULT, FULL_MIN);
        assert_eq!(r, b(740, 0, 440, 640));
    }

    #[test]
    fn oversized_saved_window_shrinks_to_its_monitor() {
        let mons = vec![mon(0, 0, 1280, 680, 1.0)];
        let r = restore(
            Some(b(0, 0, 3000, 2000)),
            &mons,
            None,
            FULL_DEFAULT,
            FULL_MIN,
        );
        assert_eq!(r, b(0, 0, 1280, 680));
    }

    #[test]
    fn zero_sized_saved_bounds_fall_back_to_default() {
        let m = mon(0, 0, 1920, 1040, 1.0);
        let r = restore(Some(b(10, 10, 0, 0)), &[m], None, FULL_DEFAULT, FULL_MIN);
        assert_eq!(r, b(740, 0, 440, 640));
    }

    #[test]
    fn layout_sizes_match_spec() {
        assert_eq!(
            layout_sizes(LayoutMode::Full),
            (LogicalSize::new(380, 520), LogicalSize::new(440, 640))
        );
        assert_eq!(
            layout_sizes(LayoutMode::Prompter).0,
            LogicalSize::new(380, 160)
        );
    }

    #[test]
    fn nonsense_scale_is_treated_as_100_percent() {
        assert_eq!(FULL_DEFAULT.to_physical(f64::NAN), (440, 640));
        assert_eq!(FULL_DEFAULT.to_physical(0.0), (440, 640));
    }
}
