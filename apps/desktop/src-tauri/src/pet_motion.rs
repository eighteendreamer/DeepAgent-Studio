use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::pet_store::{Position, Rect};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Request {
    pub(super) prefer_right: bool,
    pub(super) duration_ms: u32,
}

pub(super) struct Roam {
    pub(super) started: Instant,
    origin: Position,
    distance: i32,
    move_end: Duration,
    last: Position,
}

impl Roam {
    // The shipped run clip turns first and finishes with a trip/recovery sequence.
    // Timing is based on the dsh-pet configuration for this specific clip.
    const LEAD: Duration = Duration::from_millis(1_750);
    const TAIL: Duration = Duration::from_millis(4_800);

    pub(super) fn plan(
        origin: Position,
        frame: Position,
        width: u32,
        area: Rect,
        scale: f64,
        request: Request,
    ) -> Option<Self> {
        let left = f64::from(frame.x) - area.x;
        let right = area.x + area.width - f64::from(frame.x) - f64::from(width);
        // Don't correct an off-screen position here: running must never snap the pet.
        if left < 0.0 || right < 0.0 {
            return None;
        }
        let work_time = Duration::from_millis(u64::from(request.duration_ms.min(12_000)));
        let move_end = work_time.checked_sub(Self::TAIL)?;
        if move_end <= Self::LEAD {
            return None;
        }
        // Keep the stride proportional to the rendered character and away from screen edges.
        let margin = 12.0 * scale;
        let ratio = f64::from(width) / 462.0;
        let min = 72.0 * ratio;
        let max = 200.0 * ratio;
        let candidates = [(-1.0, left), (1.0, right)];
        let preferred = usize::from(request.prefer_right);
        let (direction, available) = [candidates[preferred], candidates[1 - preferred]]
            .into_iter()
            .find(|(_, room)| *room - margin >= min)?;
        Some(Self {
            started: Instant::now(),
            origin,
            distance: (direction * (available - margin).min(max).floor()) as i32,
            move_end,
            last: origin,
        })
    }

    pub(super) fn goes_right(&self) -> bool {
        self.distance > 0
    }

    pub(super) fn advance(&mut self, elapsed: Duration) -> (Option<Position>, bool) {
        let motion_time = (self.move_end - Self::LEAD).as_secs_f64();
        let fraction = (elapsed.saturating_sub(Self::LEAD).as_secs_f64() / motion_time).min(1.0);
        let progress = fraction * fraction * (3.0 - 2.0 * fraction);
        let next = Position {
            x: self.origin.x + (f64::from(self.distance) * progress).round() as i32,
            y: self.origin.y,
        };
        let changed = (next != self.last).then_some(next);
        self.last = next;
        (changed, elapsed >= self.move_end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect {
        x: -1600.0,
        y: 0.0,
        width: 1600.0,
        height: 900.0,
    };

    #[test]
    fn switches_direction_at_the_edge_and_moves_only_horizontally() {
        let frame = Position { x: -320, y: 300 };
        let origin = Position { x: -580, y: 220 };
        let request = Request {
            prefer_right: true,
            duration_ms: 10_000,
        };
        let mut plan = Roam::plan(origin, frame, 300, SCREEN, 1.25, request).unwrap();
        assert!(!plan.goes_right());
        assert_eq!(plan.advance(Duration::from_millis(1_750)), (None, false));
        assert_eq!(
            plan.advance(Duration::from_millis(3_475)),
            (Some(Position { x: -645, y: 220 }), false)
        );
        assert_eq!(
            plan.advance(Duration::from_millis(5_200)),
            (Some(Position { x: -709, y: 220 }), true)
        );
    }

    #[test]
    fn does_not_snap_or_cross_work_area_when_there_is_no_room() {
        let request = || Request {
            prefer_right: false,
            duration_ms: 10_000,
        };
        let frame = Position { x: -1620, y: 300 };
        assert!(Roam::plan(frame, frame, 300, SCREEN, 1.0, request()).is_none());
        let frame = Position { x: -1570, y: 300 };
        assert!(Roam::plan(frame, frame, 1540, SCREEN, 1.0, request()).is_none());
    }
}
