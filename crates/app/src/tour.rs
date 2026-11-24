//! The editor tour: one part of the window at a time is lit up and explained,
//! while the rest dims. The tour only marks where the parts are, so the layout
//! of the editor stays exactly as it is without it.

use std::time::{Duration, Instant};

/// A part of the window that a step points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TourAnchor {
    FileTree,
    Editor,
    Toolbar,
    Recompile,
    Pdf,
    Logs,
    FileMenu,
    Home,
    HelpMenu,
}

pub struct Step {
    pub anchor: TourAnchor,
    pub title: &'static str,
    pub body: &'static str,
}

pub const STEPS: &[Step] = &[
    Step {
        anchor: TourAnchor::FileTree,
        title: "Your files",
        body: "Everything in this project. Click a file to open it. The buttons on top add files, folders and images.",
    },
    Step {
        anchor: TourAnchor::Editor,
        title: "Write here",
        body: if cfg!(windows) { "This is your document. Type, or paste the LaTeX that Claude wrote for you. Ctrl+S saves and updates the PDF." } else { "This is your document. Type, or paste the LaTeX that Claude wrote for you. ⌘S saves and updates the PDF." },
    },
    Step {
        anchor: TourAnchor::Toolbar,
        title: "Insert without code",
        body: "Bold, lists, formulas, tables, figures and citations. These buttons write the code for you.",
    },
    Step {
        anchor: TourAnchor::Recompile,
        title: "Make the PDF",
        body: "Recompile turns your text into a PDF. The arrow next to it sets when this happens by itself.",
    },
    Step {
        anchor: TourAnchor::Pdf,
        title: "Your PDF",
        body: if cfg!(windows) { "The result, next to what you write. Ctrl-click a spot to jump to its line in the text." } else { "The result, next to what you write. ⌘-click a spot to jump to its line in the text." },
    },
    Step {
        anchor: TourAnchor::Logs,
        title: "When something breaks",
        body: "If the PDF cannot be made, a card above it says why and on which line, in plain words. This button lists every message.",
    },
    Step {
        anchor: TourAnchor::FileMenu,
        title: "Take your work along",
        body: "The File menu downloads the PDF, the source as a zip and drafts for Word.",
    },
    Step {
        anchor: TourAnchor::Home,
        title: "All your projects",
        body: "This leads back to your projects, the templates and the writing tools.",
    },
    Step {
        anchor: TourAnchor::HelpMenu,
        title: "That's it",
        body: "Help, Editor tour plays this again whenever you like.",
    },
];

/// How long the light takes to move from one part to the next.
const GLIDE: Duration = Duration::from_millis(280);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    pub fn grow(&self, by: f32) -> Rect {
        Rect { x: self.x - by, y: self.y - by, width: self.width + 2. * by, height: self.height + 2. * by }
    }

    /// Kept inside a window of `width` by `height`.
    pub fn clamp(&self, width: f32, height: f32) -> Rect {
        let x = self.x.clamp(0., width);
        let y = self.y.clamp(0., height);
        Rect { x, y, width: (self.right().min(width) - x).max(0.), height: (self.bottom().min(height) - y).max(0.) }
    }

    fn lerp(&self, to: &Rect, t: f32) -> Rect {
        let mix = |a: f32, b: f32| a + (b - a) * t;
        Rect { x: mix(self.x, to.x), y: mix(self.y, to.y), width: mix(self.width, to.width), height: mix(self.height, to.height) }
    }
}

pub struct Tour {
    pub step: usize,
    from: Rect,
    to: Option<Rect>,
    started: Instant,
}

impl Default for Tour {
    fn default() -> Self {
        Self { step: 0, from: Rect::default(), to: None, started: Instant::now() }
    }
}

impl Tour {
    pub fn current(&self) -> &'static Step {
        &STEPS[self.step.min(STEPS.len() - 1)]
    }

    pub fn is_last(&self) -> bool {
        self.step + 1 >= STEPS.len()
    }

    /// Returns false when there is no next step, so the tour ends.
    pub fn next(&mut self) -> bool {
        if self.is_last() {
            return false;
        }
        self.step += 1;
        true
    }

    pub fn back(&mut self) {
        self.step = self.step.saturating_sub(1);
    }

    /// Where the light is now, gliding toward `target`. The second value says
    /// whether it is still moving, so that another frame is needed.
    pub fn light(&mut self, target: Rect) -> (Rect, bool) {
        let now = Instant::now();
        match self.to {
            None => {
                self.from = target;
                self.to = Some(target);
                return (target, false);
            }
            Some(to) if to != target => {
                self.from = self.position(now);
                self.to = Some(target);
                self.started = now;
            }
            _ => {}
        }
        let moving = now.duration_since(self.started) < GLIDE;
        (self.position(now), moving)
    }

    fn position(&self, now: Instant) -> Rect {
        let Some(to) = self.to else {
            return self.from;
        };
        let t = (now.duration_since(self.started).as_secs_f32() / GLIDE.as_secs_f32()).min(1.);
        // Fast at first, then settling.
        let eased = 1. - (1. - t).powi(3);
        self.from.lerp(&to, eased)
    }
}

/// Where the explanation goes: beside the lit part where there is room, else
/// below or above it, and always inside the window.
pub fn place_bubble(target: Rect, bubble: (f32, f32), window: (f32, f32), gap: f32) -> (f32, f32) {
    let (width, height) = bubble;
    let (window_width, window_height) = window;
    let fits_x = |x: f32| x >= gap && x + width <= window_width - gap;
    let fits_y = |y: f32| y >= gap && y + height <= window_height - gap;
    let clamp_x = |x: f32| x.clamp(gap, (window_width - width - gap).max(gap));
    let clamp_y = |y: f32| y.clamp(gap, (window_height - height - gap).max(gap));
    let beside_y = clamp_y(target.y);
    let candidates = [
        (target.right() + gap, beside_y),
        (target.x - gap - width, beside_y),
        (clamp_x(target.x), target.bottom() + gap),
        (clamp_x(target.x), target.y - gap - height),
    ];
    candidates
        .into_iter()
        .find(|&(x, y)| fits_x(x) && fits_y(y))
        // The part fills the window, so the explanation sits inside it, at the bottom.
        .unwrap_or((clamp_x(target.x + (target.width - width) / 2.), clamp_y(target.bottom() - height - gap)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect { x, y, width, height }
    }

    #[test]
    fn steps_stop_at_both_ends() {
        let mut tour = Tour::default();
        tour.back();
        assert_eq!(tour.step, 0);
        for _ in 1..STEPS.len() {
            assert!(tour.next());
        }
        assert!(tour.is_last());
        assert!(!tour.next());
        assert_eq!(tour.step, STEPS.len() - 1);
        tour.back();
        assert_eq!(tour.step, STEPS.len() - 2);
    }

    #[test]
    fn the_bubble_sits_beside_below_or_inside() {
        let window = (1400., 900.);
        // A narrow panel on the left: to its right.
        assert_eq!(place_bubble(rect(40., 40., 200., 800.), (320., 160.), window, 12.), (252., 40.));
        // A pane on the right edge: to its left.
        assert_eq!(place_bubble(rect(1100., 80., 300., 800.), (320., 160.), window, 12.), (768., 80.));
        // A wide bar on top: below it.
        assert_eq!(place_bubble(rect(0., 0., 1400., 40.), (320., 160.), window, 12.), (12., 52.));
        // Everything: inside, at the bottom, and still in the window.
        let (x, y) = place_bubble(rect(0., 0., 1400., 900.), (320., 160.), window, 12.);
        assert!(x >= 12. && x + 320. <= 1388. && y + 160. <= 888.);
    }

    #[test]
    fn the_light_glides_and_then_rests() {
        let mut tour = Tour::default();
        let first = rect(0., 0., 100., 100.);
        assert_eq!(tour.light(first), (first, false));
        let second = rect(500., 0., 100., 100.);
        let (now, moving) = tour.light(second);
        assert!(moving);
        assert!(now.x < 500.);
        tour.started -= GLIDE;
        assert_eq!(tour.light(second), (second, false));
    }

    #[test]
    fn rects_stay_in_the_window() {
        assert_eq!(rect(-10., 5., 50., 2000.).clamp(100., 100.), rect(0., 5., 40., 95.));
    }
}
