//! The deletion a tracked cut left behind, kept on the session for the paste right after it,
//! which `cmd::moves` turns into a tracked move (`w:moveFrom`/`w:moveTo`).

/// The deletion the last tracked cut recorded, which the paste right after it makes a move.
#[derive(Clone, Debug)]
pub struct PendingMove {
    /// Its revision (made for this cut alone).
    pub rev: u32,
    /// The text that was cut.
    pub text: String,
    pub author: String,
}
