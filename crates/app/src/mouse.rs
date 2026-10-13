//! The menu mouse beside the keyboard and pads.
//!
//! * The cursor takes over only once it really moves ([`MOVE_PX`] from where
//!   it came to rest): a stray event while it rests (window focus, a
//!   compositor re-sending the position) neither shows it nor moves the focus.
//! * A key or pad press always wins: the cursor hides and forgets what it
//!   hovered until it moves again (a click wakes it where it is).
//! * It hides after [`IDLE_TICKS`] without movement, and in fights (the
//!   shell only shows it in menus).

/// Window pixels the cursor must travel from rest before it takes over.
pub const MOVE_PX: f32 = 3.0;
/// Ticks without movement before the cursor hides (3 s).
pub const IDLE_TICKS: u64 = 180;

/// What the mouse may do right now (see the module docs).
#[derive(Debug, Default)]
pub struct MouseGate {
    /// Last known cursor position (window pixels).
    pos: Option<(f32, f32)>,
    /// Where the cursor came to rest (measured from while asleep).
    anchor: Option<(f32, f32)>,
    /// The mouse drives the menus (visible, hover moves the focus).
    active: bool,
    /// Tick of the last real movement.
    last_move: u64,
}

impl MouseGate {
    /// The cursor moved to `(x, y)` at tick `now`. True when the move should
    /// reach the screen (hover); false while it sleeps and has not travelled
    /// [`MOVE_PX`] from rest.
    pub fn moved(&mut self, x: f32, y: f32, now: u64) -> bool {
        self.pos = Some((x, y));
        if self.active {
            self.last_move = now;
            return true;
        }
        let Some((ax, ay)) = self.anchor else {
            self.anchor = Some((x, y));
            return false;
        };
        if (x - ax).abs().max((y - ay).abs()) >= MOVE_PX {
            self.active = true;
            self.last_move = now;
            return true;
        }
        false
    }

    /// A click or a wheel turn at tick `now`: an intentional mouse action
    /// that wakes it. Returns the position to hover first when it was asleep.
    pub fn pressed(&mut self, now: u64) -> Option<(f32, f32)> {
        let woke = !self.active;
        self.active = true;
        self.last_move = now;
        if woke { self.pos } else { None }
    }

    /// A key or pad press: the keyboard takes over. True when the mouse was
    /// driving (the caller makes the screen forget the hover).
    pub fn keyboard(&mut self) -> bool {
        self.sleep()
    }

    /// The cursor left the window.
    pub fn left(&mut self) {
        self.pos = None;
        self.anchor = None;
        self.active = false;
    }

    /// Once per tick: true when the mouse just went idle (the caller hides
    /// the cursor and drops the hover).
    pub fn tick(&mut self, now: u64) -> bool {
        self.active && now.saturating_sub(self.last_move) >= IDLE_TICKS && self.sleep()
    }

    /// The cursor should be on screen (in a menu).
    pub fn visible(&self) -> bool {
        self.active
    }

    fn sleep(&mut self) -> bool {
        let was = self.active;
        self.active = false;
        self.anchor = self.pos;
        was
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resting_cursor_never_takes_over() {
        let mut g = MouseGate::default();
        // the first event only says where it rests
        assert!(!g.moved(100.0, 100.0, 1));
        assert!(!g.visible());
        // jitter while it rests
        assert!(!g.moved(101.0, 102.0, 2));
        assert!(!g.moved(100.0, 100.0, 3));
        assert!(!g.visible());
        // a real move
        assert!(g.moved(104.0, 100.0, 4));
        assert!(g.visible());
        assert!(g.moved(104.5, 100.0, 5), "then every move hovers");
    }

    #[test]
    fn keys_win_and_the_mouse_needs_a_new_move() {
        let mut g = MouseGate::default();
        g.moved(10.0, 10.0, 1);
        assert!(g.moved(50.0, 10.0, 2));
        assert!(g.keyboard(), "it was driving");
        assert!(!g.visible());
        assert!(!g.keyboard());
        // a nudge is not enough, a move is
        assert!(!g.moved(51.0, 11.0, 3));
        assert!(g.moved(60.0, 11.0, 4));
        // a click wakes it where it is
        g.keyboard();
        assert_eq!(g.pressed(5), Some((60.0, 11.0)));
        assert!(g.visible());
        assert_eq!(g.pressed(6), None, "already awake");
    }

    #[test]
    fn it_hides_when_idle() {
        let mut g = MouseGate::default();
        g.moved(0.0, 0.0, 0);
        g.moved(20.0, 0.0, 10);
        assert!(!g.tick(10 + IDLE_TICKS - 1));
        assert!(g.tick(10 + IDLE_TICKS));
        assert!(!g.visible());
        assert!(!g.tick(10 + IDLE_TICKS + 1));
        assert!(!g.moved(21.0, 0.0, 400), "rests where it hid");
        assert!(g.moved(30.0, 0.0, 401));
        g.left();
        assert!(!g.visible());
        assert!(!g.moved(500.0, 500.0, 402), "re-entering only rests");
    }
}
