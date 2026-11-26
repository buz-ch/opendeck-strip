//! What every line of the `zj-status-export` stream shares: the protocol version,
//! the ways a line can be refused and the `seq` numbering.

/// Line versions this build understands. A higher `v` is refused, never guessed at.
pub const KNOWN_VERSION: u64 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Not a line this build can read: log it and read the next one.
    Skip,
    /// The producer speaks a newer protocol; stop and exit 2.
    TooOld,
}

/// Where a stream's numbering stands, to notice a gap or a reset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Seq {
    /// No numbered line seen yet.
    #[default]
    Start,
    /// The next numbered line must carry this `seq`.
    Next(u64),
    /// A line was dropped or unparsable; the next numbered line cannot be trusted.
    Lost,
}

impl Seq {
    /// Feed one line's `seq` (`None` for lines without one); true when the stream
    /// skipped or repeated a number, so a line in between may have been lost.
    pub fn check(&mut self, seq: Option<u64>) -> bool {
        let Some(seq) = seq else {
            return false;
        };
        let (next, gap) = match *self {
            Self::Start => (seq.saturating_add(1), false),
            Self::Next(expected) => (seq.saturating_add(1), seq != expected),
            Self::Lost => (seq.saturating_add(1), true),
        };
        *self = Self::Next(next);
        gap
    }

    /// A line was received but could not be read; its number is unknown.
    pub fn lost(&mut self) {
        if matches!(self, Self::Next(_)) {
            *self = Self::Lost;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seq_notices_a_gap_only_after_a_numbered_line() {
        let mut seq = Seq::default();
        assert!(!seq.check(Some(0)));
        assert!(!seq.check(Some(1)));
        assert!(
            !seq.check(None),
            "a line without seq leaves the numbering alone"
        );
        assert!(seq.check(Some(3)), "a gap");
        assert!(!seq.check(Some(4)), "tracking resumes after the gap");
        assert!(seq.check(Some(4)), "a repeated number");
        seq.lost();
        assert!(
            seq.check(Some(5)),
            "an unreadable line hides the next number"
        );
        assert!(!seq.check(Some(6)));
        let mut fresh = Seq::default();
        fresh.lost();
        assert!(!fresh.check(Some(0)), "junk before hello is not a gap");
    }
}
