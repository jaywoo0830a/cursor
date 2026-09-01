//! Pointer-source state machine — "block the system cursor whenever the pen
//! is involved, including the transition moments".
//!
//! The system cursor tends to "pop out" at the exact moment the pointer
//! source changes *and that change involves the pen*: pen down↔pen up,
//! pen→mouse, mouse→pen, pen→pad, pad→pen. This module models the pointer
//! source (Mouse / Pen hover / Pen down / Pad / Unknown), detects those
//! transitions and says whether the system cursor should be suppressed
//! (hidden, with our painted circle shown instead) for the current frame.
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
}

/// Filtered pointer-source state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerFilter {
    state: PointerSource,
    /// Frames remaining in the post-transition suppression hold.
    hold_frames: u32,
    /// Frames since the last event of any kind (idle timeout).
    idle_frames: u32,
}

impl PointerFilter {
    /// Frames to keep the cursor suppressed after a pen-involving transition
    /// (~200 ms at 60 fps) so the system cursor can't pop out.
    pub const HOLD_FRAMES: u32 = 12;
    /// After this many frames with no event at all, drop back to `Unknown`
    /// so a stale pen state can't keep the system cursor hidden forever.
    pub const IDLE_FRAMES: u32 = 90;

    /// Create a fresh filter (starts in `Unknown`).
    pub fn new() -> Self {
        Self {
            state: PointerSource::Unknown,
            hold_frames: 0,
            idle_frames: 0,
        }
    }

    /// Current pointer source.
    pub fn state(&self) -> PointerSource {
        self.state
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
        if let Some(p) = pen {
            next = if p.down {
                PointerSource::PenDown
            } else {
                PointerSource::PenHover
            };
        } else if mouse {
            next = PointerSource::Mouse;
        } else if pad {
            next = PointerSource::Pad;
        } else if self.idle_frames >= Self::IDLE_FRAMES {
            next = PointerSource::Unknown;
        }

        if transition_involves_pen(prev, next) {
            self.hold_frames = Self::HOLD_FRAMES;
        } else if self.hold_frames > 0 {
            self.hold_frames -= 1;
        }

        self.state = next;
        should_suppress(self.state, self.hold_frames)
    }
}

/// Pure decision: does this state change involve the pen? (Pen down↔up,
/// pen↔mouse, pen↔pad, …) If so, the cursor must be held suppressed so it
/// can't pop out at the transition.
fn transition_involves_pen(prev: PointerSource, next: PointerSource) -> bool {
    prev.is_pen() || next.is_pen()
}

/// Pure decision: suppress the system cursor while in a pen state, or while
/// the post-transition hold is still active.
fn should_suppress(state: PointerSource, hold_frames: u32) -> bool {
    state.is_pen() || hold_frames > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(down: bool, in_range: bool) -> Option<PenSignal> {
        Some(PenSignal { down, in_range })
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
                assert_eq!(
                    transition_involves_pen(from, to),
                    from.is_pen() || to.is_pen(),
                    "from={from:?} to={to:?}"
                );
            }
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
        assert!(f.update(p(true, true), false, false)); // PenDown
        assert!(f.update(p(false, true), false, false)); // PenHover
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
        assert!(f.update(p(true, true), false, false)); // → PenDown
    }

    #[test]
    fn pen_down_up_stays_suppressed() {
        let mut f = PointerFilter::new();
        f.update(p(true, true), false, false); // PenDown
        for _ in 0..PointerFilter::HOLD_FRAMES + 5 {
            assert!(f.update(p(false, true), false, false)); // PenHover (up)
        }
    }

    #[test]
    fn pen_to_mouse_holds_then_releases() {
        let mut f = PointerFilter::new();
        f.update(p(true, true), false, false); // PenDown
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
        f.update(p(true, true), false, false); // PenDown
        assert!(f.update(None, false, true)); // → Pad: pen involved → hold
    }

    #[test]
    fn pad_to_pen_holds() {
        let mut f = PointerFilter::new();
        f.update(None, false, true); // Pad
        assert!(f.update(p(false, true), false, false)); // → PenHover
    }

    #[test]
    fn idle_timeout_releases() {
        let mut f = PointerFilter::new();
        f.update(p(true, true), false, false); // PenDown
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
