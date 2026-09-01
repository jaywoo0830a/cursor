//! Pointer-source state machine — "block the system cursor whenever the pen
//! is involved, including the transition moments".
//!
//! The system cursor tends to "pop out" at the exact moment the pointer
//! source changes *and that change involves the pen*: pen down↔pen up,
//! pen→mouse, mouse→pen, pen→pad, pad→pen — as well as **pen-internal**
//! changes like pressing/releasing the barrel (side) button or the eraser.
//!
//! This module models the pointer source (Mouse / Pen hover / Pen down /
//! Pad / Unknown), detects all those transitions and decides:
//!
//! * `suppress` — hide the system cursor (and paint our circle instead) while
//!   in a pen state, and for a configurable **hold** (default ~3 s) after any
//!   pen-involving transition so it can't pop out.
//! * `boost` — right after a transition, the cursor guard should run at a
//!   **higher frequency** to win any contention with the app/driver that
//!   briefly takes the cursor over.
//!
//! It is deliberately a pure, frame-driven state machine so every
//! transition combination can be unit-tested.

/// The pointer source at a given moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerSource {
    /// Mouse / mouse-like input.
    Mouse,
    /// Pen hovering (in range, not touching).
    PenHover,
    /// Pen touching the surface (writing / drawing).
    PenDown,
    /// Touchpad / touch screen.
    Pad,
    /// No recent signal.
    Unknown,
}

impl PointerSource {
    /// Is this a pen-related state?
    pub fn is_pen(self) -> bool {
        matches!(self, Self::PenHover | Self::PenDown)
    }
}

/// A single pen-contact signal fed to the filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PenSignal {
    /// Tip switch / contact bit.
    pub down: bool,
    /// Pen in range bit.
    pub in_range: bool,
    /// Barrel / side button pressed.
    pub barrel: bool,
    /// Eraser button pressed.
    pub eraser: bool,
}

/// Filtered pointer-source state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerFilter {
    state: PointerSource,
    /// Frames remaining in the post-transition suppression hold.
    hold_frames: u32,
    /// Target hold length (frames) applied to future pen transitions.
    hold_max: u32,
    /// Frames remaining in the cursor-guard boost window.
    boost_frames: u32,
    /// Frames since the last event of any kind (idle timeout).
    idle_frames: u32,
    /// Last seen pen button state `(barrel, eraser)` — a change is a
    /// pen-internal transition that must hold the suppression.
    last_pen_buttons: (bool, bool),
}

impl PointerFilter {
    /// Default suppression hold after a pen-involving transition: ~3 s at
    /// 60 fps.
    pub const HOLD_FRAMES: u32 = 180;
    /// How many frames the cursor guard runs at boosted frequency right
    /// after a pen transition (~0.5 s at 60 fps).
    pub const BOOST_FRAMES: u32 = 30;
    /// After this many frames with no event at all, drop back to `Unknown`
    /// so a stale pen state can't keep the system cursor hidden forever.
    pub const IDLE_FRAMES: u32 = 90;

    /// Create a fresh filter with the default hold.
    pub fn new() -> Self {
        Self::with_hold(Self::HOLD_FRAMES)
    }

    /// Create a fresh filter with a custom suppression hold (in frames).
    pub fn with_hold(hold_frames: u32) -> Self {
        Self {
            state: PointerSource::Unknown,
            hold_frames: 0,
            hold_max: hold_frames,
            boost_frames: 0,
            idle_frames: 0,
            last_pen_buttons: (false, false),
        }
    }

    /// Change the suppression hold length (in frames); applies to future
    /// pen transitions.
    pub fn set_hold(&mut self, hold_frames: u32) {
        self.hold_max = hold_frames;
    }

    /// Current pointer source.
    pub fn state(&self) -> PointerSource {
        self.state
    }

    /// Is the post-transition suppression hold currently active?
    pub fn in_hold(&self) -> bool {
        self.hold_frames > 0
    }

    /// Should the cursor guard run at boosted frequency right now (right
    /// after a pen-involving transition)?
    pub fn in_boost(&self) -> bool {
        self.boost_frames > 0
    }

    /// Feed one frame of raw signals and return whether the system cursor
    /// should be suppressed.
    ///
    /// * `pen` — `Some` if a pen report arrived this frame.
    /// * `mouse` — a mouse event arrived this frame.
    /// * `pad` — a touchpad/touch event arrived this frame.
    pub fn update(&mut self, pen: Option<PenSignal>, mouse: bool, pad: bool) -> bool {
        let had_event = pen.is_some() || mouse || pad;
        self.idle_frames = if had_event {
            0
        } else {
            self.idle_frames.saturating_add(1)
        };

        let prev = self.state;
        let mut next = prev;
        let mut pen_buttons_changed = false;
        if let Some(p) = pen {
            next = if p.down {
                PointerSource::PenDown
            } else {
                PointerSource::PenHover
            };
            let buttons = (p.barrel, p.eraser);
            pen_buttons_changed = buttons != self.last_pen_buttons;
            self.last_pen_buttons = buttons;
        } else if mouse {
            next = PointerSource::Mouse;
        } else if pad {
            next = PointerSource::Pad;
        } else if self.idle_frames >= Self::IDLE_FRAMES {
            next = PointerSource::Unknown;
        }

        if !next.is_pen() {
            // The pen is not the active source: clear any stale pen-button
            // state so a re-entry doesn't misfire.
            self.last_pen_buttons = (false, false);
        }

        // Suppress on any pen-involving source transition, and also on
        // pen-internal changes (barrel / eraser button press or release).
        let pen_involved = transition_involves_pen(prev, next) || pen_buttons_changed;
        if pen_involved {
            self.hold_frames = self.hold_max;
            self.boost_frames = Self::BOOST_FRAMES;
        } else {
            if self.hold_frames > 0 {
                self.hold_frames -= 1;
            }
            if self.boost_frames > 0 {
                self.boost_frames -= 1;
            }
        }

        self.state = next;
        should_suppress(self.state, self.hold_frames)
    }
}

/// Pure decision: does this **actual state change** involve the pen? (Pen
/// down↔up, pen↔mouse, pen↔pad, …) A steady state (prev == next) is not a
/// transition, so it never re-arms the hold/boost. If so, the cursor must be
/// held suppressed so it can't pop out at the transition.
fn transition_involves_pen(prev: PointerSource, next: PointerSource) -> bool {
    prev != next && (prev.is_pen() || next.is_pen())
}

/// Pure decision: suppress the system cursor while in a pen state, or while
/// the post-transition hold is still active.
fn should_suppress(state: PointerSource, hold_frames: u32) -> bool {
    state.is_pen() || hold_frames > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pen event with `down`, `barrel`, `eraser` (in range by default).
    fn p(down: bool, barrel: bool, eraser: bool) -> Option<PenSignal> {
        Some(PenSignal {
            down,
            in_range: true,
            barrel,
            eraser,
        })
    }

    #[test]
    fn every_pen_involving_transition_is_detected() {
        let sources = [
            PointerSource::Mouse,
            PointerSource::PenHover,
            PointerSource::PenDown,
            PointerSource::Pad,
        ];
        for &from in &sources {
            for &to in &sources {
                let expected = from != to && (from.is_pen() || to.is_pen());
                assert_eq!(
                    transition_involves_pen(from, to),
                    expected,
                    "from={from:?} to={to:?}"
                );
            }
        }
    }

    #[test]
    fn steady_state_is_not_a_transition() {
        for &s in &[
            PointerSource::Mouse,
            PointerSource::PenHover,
            PointerSource::PenDown,
            PointerSource::Pad,
            PointerSource::Unknown,
        ] {
            assert!(!transition_involves_pen(s, s), "{s:?} → {s:?} is not a transition");
        }
    }

    #[test]
    fn suppress_decision_per_state_and_hold() {
        assert!(should_suppress(PointerSource::PenDown, 0));
        assert!(should_suppress(PointerSource::PenHover, 0));
        assert!(!should_suppress(PointerSource::Mouse, 0));
        assert!(!should_suppress(PointerSource::Pad, 0));
        assert!(!should_suppress(PointerSource::Unknown, 0));
        assert!(should_suppress(PointerSource::Mouse, 1)); // post-transition hold
    }

    #[test]
    fn pen_down_and_hover_suppress() {
        let mut f = PointerFilter::new();
        assert!(f.update(p(true, false, false), false, false)); // PenDown
        assert!(f.update(p(false, false, false), false, false)); // PenHover
    }

    #[test]
    fn mouse_and_pad_do_not_suppress() {
        let mut f = PointerFilter::new();
        assert!(!f.update(None, true, false)); // Mouse
        assert!(!f.update(None, false, true)); // Pad
    }

    #[test]
    fn mouse_to_pen_suppresses_immediately() {
        let mut f = PointerFilter::new();
        f.update(None, true, false); // Mouse
        assert!(f.update(p(true, false, false), false, false)); // → PenDown
    }

    #[test]
    fn pen_down_up_stays_suppressed() {
        let mut f = PointerFilter::new();
        f.update(p(true, false, false), false, false); // PenDown
        for _ in 0..PointerFilter::HOLD_FRAMES + 5 {
            assert!(f.update(p(false, false, false), false, false)); // PenHover (up)
        }
    }

    #[test]
    fn pen_to_mouse_holds_then_releases() {
        let mut f = PointerFilter::new();
        f.update(p(true, false, false), false, false); // PenDown
        assert!(f.update(None, true, false)); // → Mouse: transition hold active
        let mut released = false;
        for _ in 0..PointerFilter::HOLD_FRAMES + 2 {
            if !f.update(None, true, false) {
                released = true;
                break;
            }
        }
        assert!(released, "suppression should release after the hold");
    }

    #[test]
    fn mouse_to_pad_does_not_hold() {
        let mut f = PointerFilter::new();
        f.update(None, true, false); // Mouse
        assert!(!f.update(None, false, true)); // → Pad: no pen involved
    }

    #[test]
    fn pad_to_mouse_does_not_hold() {
        let mut f = PointerFilter::new();
        f.update(None, false, true); // Pad
        assert!(!f.update(None, true, false)); // → Mouse: no pen involved
    }

    #[test]
    fn pen_to_pad_holds() {
        let mut f = PointerFilter::new();
        f.update(p(true, false, false), false, false); // PenDown
        assert!(f.update(None, false, true)); // → Pad: pen involved → hold
    }

    #[test]
    fn pad_to_pen_holds() {
        let mut f = PointerFilter::new();
        f.update(None, false, true); // Pad
        assert!(f.update(p(false, false, false), false, false)); // → PenHover
    }

    #[test]
    fn barrel_press_while_hovering_holds() {
        let mut f = PointerFilter::new();
        f.update(p(false, false, false), false, false); // PenHover, no button
        assert!(f.update(p(false, true, false), false, false)); // barrel press
    }

    #[test]
    fn barrel_release_while_hovering_holds() {
        let mut f = PointerFilter::new();
        f.update(p(false, true, false), false, false); // PenHover + barrel
        assert!(f.update(p(false, false, false), false, false)); // release
    }

    #[test]
    fn barrel_press_with_down_holds() {
        let mut f = PointerFilter::new();
        f.update(p(true, false, false), false, false); // PenDown
        assert!(f.update(p(true, true, false), false, false)); // barrel while writing
    }

    #[test]
    fn eraser_press_holds() {
        let mut f = PointerFilter::new();
        f.update(p(false, false, false), false, false); // PenHover
        assert!(f.update(p(false, false, true), false, false)); // eraser
    }

    #[test]
    fn no_button_repeat_does_not_spuriously_hold() {
        let mut f = PointerFilter::new();
        f.update(p(false, false, false), false, false); // PenHover
        for _ in 0..PointerFilter::HOLD_FRAMES + 2 {
            assert!(f.update(p(false, false, false), false, false));
        }
    }

    #[test]
    fn boost_active_after_pen_transition_then_decays() {
        let mut f = PointerFilter::new();
        f.update(None, true, false); // Mouse
        f.update(p(true, false, false), false, false); // → PenDown: transition → boost
        assert!(f.in_boost());
        for _ in 0..PointerFilter::BOOST_FRAMES + 1 {
            f.update(p(true, false, false), false, false); // steady PenDown
        }
        assert!(!f.in_boost());
        assert!(f.in_hold()); // still suppressing (pen state)
    }

    #[test]
    fn no_pen_transition_means_no_boost() {
        let mut f = PointerFilter::new();
        f.update(None, true, false); // Mouse
        assert!(!f.in_boost());
        f.update(None, false, true); // → Pad (no pen)
        assert!(!f.in_boost());
    }

    #[test]
    fn custom_hold_length_is_respected() {
        let mut f = PointerFilter::with_hold(5);
        f.update(p(true, false, false), false, false); // PenDown (transition)
        assert!(f.update(None, true, false)); // → Mouse: hold = 5
        assert!(f.in_hold());
        for _ in 0..4 {
            assert!(f.in_hold());
            f.update(None, true, false);
        }
        f.update(None, true, false); // hold reaches 0
        assert!(!f.in_hold());
        assert!(!f.update(None, true, false)); // released
    }

    #[test]
    fn default_hold_is_three_seconds_of_frames() {
        assert_eq!(PointerFilter::new().hold_max, 180); // ~3 s at 60 fps
        assert_eq!(PointerFilter::HOLD_FRAMES, 180);
    }

    #[test]
    fn idle_timeout_releases() {
        let mut f = PointerFilter::new();
        f.update(p(true, false, false), false, false); // PenDown
        let mut released = false;
        for _ in 0..PointerFilter::IDLE_FRAMES + PointerFilter::HOLD_FRAMES + 4 {
            if !f.update(None, false, false) {
                released = true;
                break;
            }
        }
        assert!(released, "stale pen state must release after idle timeout");
    }
}
