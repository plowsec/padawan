//! Size caps. Decoders reject anything above; producers stay below.

/// Largest file the editor opens.
pub const MAX_OPEN_FILE: usize = 64 << 20;
/// Largest frame payload: one file plus headroom.
pub const MAX_FRAME: usize = MAX_OPEN_FILE + (1 << 20);
/// Path length in bytes.
pub const MAX_PATH: usize = 4096;
/// One path component.
pub const MAX_NAME: usize = 255;
/// Query strings, messages, names.
pub const MAX_TEXT: usize = 64 << 10;
/// One line of search context sent to the GUI.
pub const MAX_SNIPPET: usize = 512;
/// Entries in one directory listing.
pub const MAX_DIR_ENTRIES: usize = 50_000;
/// Items in any list reply (hits, sites, matches, lens rows).
pub const MAX_ITEMS: usize = 100_000;
