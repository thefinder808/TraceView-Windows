//! Error Lookup reads CMTrace.exe as plain bytes, so no file may ever make the
//! reader panic. The reader uses only the standard library, so it's included
//! here as it is, as `tv-errors` itself only builds on Windows.

#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../crates/tv-errors/src/resources.rs"]
mod resources;

fuzz_target!(|data: &[u8]| {
    // The input as a whole file.
    if let Some(table) = resources::cmtrace_messages(data) {
        for code in [0, 1, 0x87D0_0215, 0x8024_0022, u32::MAX] {
            let _ = resources::find_message(table, code);
        }
    }
    // The input as a message table, with a code from its first four bytes.
    if let Some((code, table)) = data.split_first_chunk::<4>() {
        let _ = resources::find_message(table, u32::from_le_bytes(*code));
        let _ = resources::find_message(data, u32::from_le_bytes(*code));
    }
});
