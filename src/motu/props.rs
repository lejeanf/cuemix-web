//! MOTU UltraLite-mk5 property table: `prop_id` ⇄ [`Param`] and payload kind.
//!
//! **Owner: `agent-motu-protocol` (props).** Source: the open-source
//! `motu-mk5-linux` project (maintainer ruling Q3). Every entry stays
//! `verified: false` until tested on real hardware by the maintainer.
//!
//! Stable interface (used by `motu::client` — do not change signatures
//! without updating `docs/ARCHITECTURE.md`):
//! - [`kind_of`]     — payload kind of a property, `None` if unknown.
//! - [`encode`]      — effective [`Param`] → wire frames (may be several, or none
//!   if the parameter has no known hardware mapping yet).
//! - [`decode`]      — hardware frame → [`Param`], `None` if unmapped.
//! - [`ENTRIES`]     — the table itself, with provenance, for docs/tests.

use super::protocol::{Frame, ProtocolError, ValueKind};
use crate::state::Param;

/// One row of the property table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropEntry {
    pub prop_id: u16,
    pub kind: ValueKind,
    /// Human description, e.g. `"mix channel fader (index = bus*64+ch)"`.
    pub name: &'static str,
    /// Where the mapping comes from (file/commit in motu-mk5-linux).
    pub source: &'static str,
    /// `true` only once confirmed on hardware by the maintainer.
    pub verified: bool,
}

/// Property table (empty until extracted).
pub const ENTRIES: &[PropEntry] = &[];

pub fn kind_of(prop_id: u16) -> Option<ValueKind> {
    ENTRIES
        .iter()
        .find(|e| e.prop_id == prop_id)
        .map(|e| e.kind)
}

/// Effective param → frames. Empty = no known hardware mapping (yet).
pub fn encode(_param: &Param) -> Result<Vec<Frame>, ProtocolError> {
    Ok(Vec::new())
}

/// Hardware frame → param. `None` = unmapped property.
pub fn decode(_frame: &Frame) -> Option<Param> {
    None
}
