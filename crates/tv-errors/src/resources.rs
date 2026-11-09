//! Reads message tables straight from the bytes of a Windows program file, with
//! no Windows calls, so the file is only ever read, never loaded or run.
//!
//! This is for `CMTrace.exe`, which explains ConfigMgr's and Windows Update's
//! codes. CMTrace is a small loader, and its message table is inside the real
//! program, which is embedded among its resources, where `FormatMessageW` can't
//! reach it.
//!
//! Every count and offset comes from the file, so each is checked before use,
//! and a damaged file only means no text. This uses nothing but the standard
//! library, so the fuzz target can include it as it is.

/// The resource type that holds message tables.
const RT_MESSAGETABLE: u32 = 11;
/// In a resource directory entry, marks a name that's a string, or a target
/// that's another directory rather than data.
const HIGH_BIT: u32 = 0x8000_0000;
/// Marks a message table entry whose text is UTF-16.
const UNICODE_TEXT: u16 = 1;

/// Where CMTrace keeps the program it runs, as a 64-bit and a 32-bit copy.
const CMTRACE_TYPE: &str = "BINRES";
const CMTRACE_PROGRAMS: [&str; 2] = ["TRACE64", "TRACE32"];

/// Finds the message table in a `CMTrace.exe`: its own, or else the one in the
/// program it embeds. None if there's neither.
pub(crate) fn cmtrace_messages(file: &[u8]) -> Option<&[u8]> {
    let resources = Resources::new(file)?;
    if let Some(table) = resources.find(Id::Number(RT_MESSAGETABLE), Id::First) {
        return Some(table);
    }
    CMTRACE_PROGRAMS.iter().find_map(|&name| {
        let program = resources.find(Id::Name(CMTRACE_TYPE), Id::Name(name))?;
        Resources::new(program)?.find(Id::Number(RT_MESSAGETABLE), Id::First)
    })
}

/// Finds a code's text in a message table, as it's stored, with nothing
/// cleaned up. None if the table has no entry for the code.
///
/// A table is a count of blocks, each a range of codes and where its entries
/// start, and then the entries: a length, which counts its own four bytes,
/// flags, and the text.
pub(crate) fn find_message(table: &[u8], code: u32) -> Option<String> {
    let blocks = u32_at(table, 0, 0)?;
    for block in 0..blocks as usize {
        // Reading past the end returns, so a huge count can't loop for long.
        let at = add(4, block.checked_mul(12)?)?;
        let low = u32_at(table, at, 0)?;
        let high = u32_at(table, at, 4)?;
        if code < low || code > high {
            continue;
        }
        let mut entry = u32_at(table, at, 8)? as usize;
        // Each entry is at least four bytes, so this ends within the table.
        for _ in low..code {
            entry = add(entry, entry_length(table, entry)?)?;
        }
        let length = entry_length(table, entry)?;
        let text = table.get(add(entry, 4)?..add(entry, length)?)?;
        let unicode = u16_at(table, entry, 2)? & UNICODE_TEXT != 0;
        return Some(if unicode {
            utf16_text(text)
        } else {
            ansi_text(text)
        });
    }
    None
}

/// An entry's length, which must at least cover its own header.
fn entry_length(table: &[u8], entry: usize) -> Option<usize> {
    let length = usize::from(u16_at(table, entry, 0)?);
    (length >= 4).then_some(length)
}

/// UTF-16 text up to its first NUL.
fn utf16_text(bytes: &[u8]) -> String {
    decode_utf16(utf16_units(bytes).take_while(|&unit| unit != 0))
}

/// Little-endian UTF-16 units. An odd last byte is left out.
fn utf16_units(bytes: &[u8]) -> impl Iterator<Item = u16> + '_ {
    let (pairs, _) = bytes.as_chunks::<2>();
    pairs.iter().map(|pair| u16::from_le_bytes(*pair))
}

/// Text from UTF-16 units, with a replacement character for any that are bad.
fn decode_utf16(units: impl Iterator<Item = u16>) -> String {
    char::decode_utf16(units)
        .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// Single-byte text up to its first NUL, read as Latin-1, which matches the
/// English code page for every letter.
fn ansi_text(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take_while(|&&byte| byte != 0)
        .map(|&byte| char::from(byte))
        .collect()
}

/// A resource's type or name.
#[derive(Clone, Copy)]
enum Id<'n> {
    Number(u32),
    /// Matched without regard to case, as Windows does.
    Name(&'n str),
    /// Whichever is listed first.
    First,
}

/// Where a section is loaded, and where its bytes are in the file.
struct Section {
    address: u32,
    raw_size: u32,
    raw_offset: u32,
}

/// A PE file's resources.
struct Resources<'a> {
    file: &'a [u8],
    sections: Vec<Section>,
    /// Where the root resource directory starts in the file. Offsets within
    /// the resources count from here.
    root: usize,
}

impl<'a> Resources<'a> {
    /// Reads a PE file's headers, as far as its resources. None if it isn't a
    /// PE file or has no resources.
    fn new(file: &'a [u8]) -> Option<Self> {
        if file.get(..2)? != b"MZ" {
            return None;
        }
        let signature = u32_at(file, 0x3C, 0)? as usize;
        if file.get(signature..add(signature, 4)?)? != b"PE\0\0" {
            return None;
        }
        let coff = add(signature, 4)?;
        let section_count = usize::from(u16_at(file, coff, 2)?);
        let optional_size = usize::from(u16_at(file, coff, 16)?);
        let optional = add(coff, 20)?;
        // The data directories follow the optional header's fixed fields,
        // which are longer in a 64-bit file.
        let (count_field, directories) = match u16_at(file, optional, 0)? {
            0x10B => (92, 96),
            0x20B => (108, 112),
            _ => return None,
        };
        // Resources are the third data directory.
        if u32_at(file, optional, count_field)? < 3 || directories + 3 * 8 > optional_size {
            return None;
        }
        let address = u32_at(file, optional, directories + 2 * 8)?;
        let sections_start = add(optional, optional_size)?;
        let sections = (0..section_count)
            .map(|index| {
                let at = add(sections_start, index * 40)?;
                Some(Section {
                    address: u32_at(file, at, 12)?,
                    raw_size: u32_at(file, at, 16)?,
                    raw_offset: u32_at(file, at, 20)?,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let mut resources = Self {
            file,
            sections,
            root: 0,
        };
        // A directory's header is 16 bytes.
        resources.root = resources.offset(address, 16)?;
        Some(resources)
    }

    /// Where `length` bytes loaded at `address` are in the file, if one section
    /// holds all of them.
    fn offset(&self, address: u32, length: u32) -> Option<usize> {
        let section = self.sections.iter().find(|section| {
            address >= section.address
                && u64::from(address - section.address) + u64::from(length)
                    <= u64::from(section.raw_size)
        })?;
        let offset = add(
            section.raw_offset as usize,
            (address - section.address) as usize,
        )?;
        self.file.get(offset..add(offset, length as usize)?)?;
        Some(offset)
    }

    /// A resource's bytes, in its first language.
    fn find(&self, kind: Id<'_>, name: Id<'_>) -> Option<&'a [u8]> {
        let names = self.subdirectory(self.root, kind)?;
        let languages = self.subdirectory(names, name)?;
        let target = self.entry(languages, Id::First)?;
        if target & HIGH_BIT != 0 {
            return None;
        }
        let data = add(self.root, target as usize)?;
        let address = u32_at(self.file, data, 0)?;
        let size = u32_at(self.file, data, 4)?;
        let start = self.offset(address, size)?;
        self.file.get(start..add(start, size as usize)?)
    }

    /// Where the directory an entry points to starts.
    fn subdirectory(&self, directory: usize, id: Id<'_>) -> Option<usize> {
        let target = self.entry(directory, id)?;
        if target & HIGH_BIT == 0 {
            return None;
        }
        add(self.root, (target & !HIGH_BIT) as usize)
    }

    /// The target of a directory's entry for an id.
    fn entry(&self, directory: usize, id: Id<'_>) -> Option<u32> {
        let named = u16_at(self.file, directory, 12)?;
        let numbered = u16_at(self.file, directory, 14)?;
        let count = usize::from(named) + usize::from(numbered);
        (0..count).find_map(|index| {
            let at = add(directory, 16 + index * 8)?;
            let name = u32_at(self.file, at, 0)?;
            let target = u32_at(self.file, at, 4)?;
            self.matches(name, id).then_some(target)
        })
    }

    fn matches(&self, name: u32, id: Id<'_>) -> bool {
        match id {
            Id::First => true,
            Id::Number(number) => name == number,
            Id::Name(wanted) => {
                name & HIGH_BIT != 0
                    && self
                        .name(name & !HIGH_BIT)
                        .is_some_and(|found| found.eq_ignore_ascii_case(wanted))
            }
        }
    }

    /// A resource's name: a UTF-16 length, then that many UTF-16 units.
    fn name(&self, offset: u32) -> Option<String> {
        let at = add(self.root, offset as usize)?;
        let length = usize::from(u16_at(self.file, at, 0)?);
        let bytes = self.file.get(add(at, 2)?..add(at, 2 + length * 2)?)?;
        Some(decode_utf16(utf16_units(bytes)))
    }
}

fn add(a: usize, b: usize) -> Option<usize> {
    a.checked_add(b)
}

fn u16_at(data: &[u8], base: usize, field: usize) -> Option<u16> {
    let start = add(base, field)?;
    let bytes = data.get(start..add(start, 2)?)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn u32_at(data: &[u8], base: usize, field: usize) -> Option<u32> {
    let start = add(base, field)?;
    let bytes = data.get(start..add(start, 4)?)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Builds a message table from blocks of consecutive codes, each entry's
    /// text and whether it's stored as UTF-16.
    fn message_table(blocks: &[(u32, &[(&str, bool)])]) -> Vec<u8> {
        let mut entries = Vec::new();
        let mut starts = Vec::new();
        let header = 4 + 12 * blocks.len();
        for (_, texts) in blocks {
            starts.push(header + entries.len());
            for &(text, unicode) in *texts {
                let mut bytes: Vec<u8> = if unicode {
                    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
                } else {
                    text.bytes().collect()
                };
                // A NUL, then padding to four bytes, as the message compiler does.
                bytes.extend([0, 0]);
                while !bytes.len().is_multiple_of(4) {
                    bytes.push(0);
                }
                entries.extend(u16::try_from(bytes.len() + 4).unwrap().to_le_bytes());
                entries.extend(u16::from(unicode).to_le_bytes());
                entries.extend(bytes);
            }
        }
        let mut table = u32::try_from(blocks.len()).unwrap().to_le_bytes().to_vec();
        for ((low, texts), start) in blocks.iter().zip(starts) {
            let high = low + u32::try_from(texts.len()).unwrap() - 1;
            for value in [*low, high, u32::try_from(start).unwrap()] {
                table.extend(value.to_le_bytes());
            }
        }
        table.extend(entries);
        table
    }

    /// A resource's type or name, as the test files are built with.
    enum Key {
        Number(u32),
        Name(&'static str),
    }

    /// A resource's name and bytes.
    type Resource<'a> = (Key, &'a [u8]);

    fn u32_of(value: usize) -> u32 {
        u32::try_from(value).unwrap()
    }

    /// Writes a resource directory: its header, then each entry's name and target.
    fn directory(section: &mut Vec<u8>, entries: &[(u32, u32)]) {
        section.extend([0u8; 12]);
        let named = entries
            .iter()
            .filter(|(name, _)| name & HIGH_BIT != 0)
            .count();
        section.extend(u16::try_from(named).unwrap().to_le_bytes());
        section.extend(u16::try_from(entries.len() - named).unwrap().to_le_bytes());
        for (name, target) in entries {
            section.extend(name.to_le_bytes());
            section.extend(target.to_le_bytes());
        }
    }

    /// Builds a 64-bit PE file with one section of resources: types, each with
    /// its resources, each in language 0x409.
    fn pe_file(types: &[(Key, &[Resource])]) -> Vec<u8> {
        const SECTION_ADDRESS: u32 = 0x1000;
        const OPTIONAL_SIZE: usize = 240;
        let raw_offset = (0x40 + 4 + 20 + OPTIONAL_SIZE + 40).next_multiple_of(0x200);

        // Lay the section out first: the root directory, a directory of names
        // for each type, one of languages for each resource, the names, the
        // data entries, then the data.
        let mut next = 16 + 8 * types.len();
        let mut name_directories = Vec::new();
        for (_, resources) in types {
            name_directories.push(next);
            next += 16 + 8 * resources.len();
        }
        let count: usize = types.iter().map(|(_, resources)| resources.len()).sum();
        let language_directories: Vec<usize> = (0..count).map(|index| next + 24 * index).collect();
        let names_at = next + 24 * count;
        let mut names = Vec::new();
        let mut id = |key: &Key| match key {
            Key::Number(number) => *number,
            Key::Name(text) => {
                let at = u32_of(names_at + names.len());
                let units: Vec<u16> = text.encode_utf16().collect();
                names.extend(u16::try_from(units.len()).unwrap().to_le_bytes());
                names.extend(units.iter().flat_map(|unit| unit.to_le_bytes()));
                at | HIGH_BIT
            }
        };
        let mut type_ids = Vec::new();
        let mut name_ids = Vec::new();
        for (kind, resources) in types {
            type_ids.push(id(kind));
            for (name, _) in *resources {
                name_ids.push(id(name));
            }
        }
        while !names.len().is_multiple_of(4) {
            names.push(0);
        }
        let data_entries_at = names_at + names.len();
        let data_at = data_entries_at + 16 * count;

        let mut section = Vec::new();
        let root: Vec<(u32, u32)> = type_ids
            .iter()
            .zip(&name_directories)
            .map(|(&kind, &at)| (kind, u32_of(at) | HIGH_BIT))
            .collect();
        directory(&mut section, &root);
        let mut resource = 0;
        for (_, resources) in types {
            let entries: Vec<(u32, u32)> = (resource..resource + resources.len())
                .map(|index| {
                    (
                        name_ids[index],
                        u32_of(language_directories[index]) | HIGH_BIT,
                    )
                })
                .collect();
            directory(&mut section, &entries);
            resource += resources.len();
        }
        for index in 0..count {
            directory(
                &mut section,
                &[(0x409, u32_of(data_entries_at + 16 * index))],
            );
        }
        section.extend(names);
        let mut data = Vec::new();
        for (_, bytes) in types.iter().flat_map(|(_, resources)| resources.iter()) {
            section.extend((SECTION_ADDRESS + u32_of(data_at + data.len())).to_le_bytes());
            section.extend(u32_of(bytes.len()).to_le_bytes());
            section.extend([0u8; 8]);
            data.extend(*bytes);
            while !data.len().is_multiple_of(4) {
                data.push(0);
            }
        }
        section.extend(data);

        let mut file = vec![0u8; raw_offset];
        file[..2].copy_from_slice(b"MZ");
        file[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        file[0x40..0x44].copy_from_slice(b"PE\0\0");
        let coff = 0x44;
        file[coff..coff + 2].copy_from_slice(&0x8664u16.to_le_bytes());
        file[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes());
        file[coff + 16..coff + 18]
            .copy_from_slice(&u16::try_from(OPTIONAL_SIZE).unwrap().to_le_bytes());
        let optional = coff + 20;
        file[optional..optional + 2].copy_from_slice(&0x20Bu16.to_le_bytes());
        file[optional + 108..optional + 112].copy_from_slice(&16u32.to_le_bytes());
        let resource_directory = optional + 112 + 2 * 8;
        file[resource_directory..resource_directory + 4]
            .copy_from_slice(&SECTION_ADDRESS.to_le_bytes());
        file[resource_directory + 4..resource_directory + 8]
            .copy_from_slice(&u32::try_from(section.len()).unwrap().to_le_bytes());
        let header = optional + OPTIONAL_SIZE;
        file[header..header + 5].copy_from_slice(b".rsrc");
        let size = u32::try_from(section.len()).unwrap();
        file[header + 8..header + 12].copy_from_slice(&size.to_le_bytes());
        file[header + 12..header + 16].copy_from_slice(&SECTION_ADDRESS.to_le_bytes());
        file[header + 16..header + 20].copy_from_slice(&size.to_le_bytes());
        file[header + 20..header + 24]
            .copy_from_slice(&u32::try_from(raw_offset).unwrap().to_le_bytes());
        file.extend(section);
        file
    }

    fn sample_table() -> Vec<u8> {
        message_table(&[
            (
                0x87D0_0213,
                &[
                    ("First", true),
                    ("Second", false),
                    ("Item not found\r\n", true),
                ],
            ),
            (0x8024_0001, &[("Windows Update text", true)]),
        ])
    }

    /// A file with just a message table.
    fn with_table(table: &[u8]) -> Vec<u8> {
        pe_file(&[(Key::Number(RT_MESSAGETABLE), &[(Key::Number(1), table)])])
    }

    /// A CMTrace-like file: a loader whose real program is embedded as a
    /// resource. Its 32-bit copy is damaged here, so only the 64-bit one works.
    fn cmtrace_like(table: &[u8]) -> Vec<u8> {
        let program = with_table(table);
        pe_file(&[
            (
                Key::Name("BINRES"),
                &[
                    (Key::Name("TRACE32"), b"not a program"),
                    (Key::Name("TRACE64"), &program),
                ],
            ),
            (Key::Number(3), &[(Key::Number(1), b"an icon")]),
        ])
    }

    #[test]
    fn codes_are_found_in_their_blocks() {
        let table = sample_table();
        assert_eq!(find_message(&table, 0x87D0_0213).as_deref(), Some("First"));
        assert_eq!(find_message(&table, 0x87D0_0214).as_deref(), Some("Second"));
        assert_eq!(
            find_message(&table, 0x87D0_0215).as_deref(),
            Some("Item not found\r\n")
        );
        assert_eq!(
            find_message(&table, 0x8024_0001).as_deref(),
            Some("Windows Update text")
        );
    }

    #[test]
    fn codes_outside_every_block_have_no_text() {
        let table = sample_table();
        for code in [
            0,
            0x87D0_0212,
            0x87D0_0216,
            0x8024_0000,
            0x8024_0002,
            u32::MAX,
        ] {
            assert_eq!(find_message(&table, code), None, "{code:#x}");
        }
    }

    #[test]
    fn single_byte_text_is_read_as_latin_1() {
        let table = message_table(&[(1, &[("caf\u{e9}", false)])]);
        // The builder wrote UTF-8, so é became two Latin-1 characters.
        assert_eq!(find_message(&table, 1).as_deref(), Some("caf\u{c3}\u{a9}"));
        let mut table = message_table(&[(1, &[("x", false)])]);
        table[4 + 12 + 4] = 0xE9;
        assert_eq!(find_message(&table, 1).as_deref(), Some("\u{e9}"));
    }

    #[test]
    fn a_damaged_table_has_no_text() {
        let table = sample_table();
        // Cut short anywhere.
        for length in 0..table.len() - 1 {
            let _ = find_message(&table[..length], 0x87D0_0215);
        }
        // An entry that claims to be shorter than its own header.
        let mut zero_length = table.clone();
        let first_entry = 4 + 2 * 12;
        zero_length[first_entry..first_entry + 2].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(find_message(&zero_length, 0x87D0_0215), None);
        // A block whose entries start past the end.
        let mut far = table.clone();
        far[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(find_message(&far, 0x87D0_0213), None);
        // Billions of blocks, but only two there: reading stops at the end.
        let mut many = table;
        many[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        let _ = find_message(&many, 0x2BAD_F00D);
    }

    #[test]
    fn a_huge_block_ends_at_the_table_end() {
        // A block claiming four billion codes, with one real entry.
        let mut table = message_table(&[(0, &[("only", true)])]);
        table[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(find_message(&table, u32::MAX), None);
    }

    #[test]
    fn a_table_is_found_in_the_file_itself() {
        let table = sample_table();
        assert_eq!(cmtrace_messages(&with_table(&table)), Some(&table[..]));
    }

    #[test]
    fn a_table_is_found_in_the_embedded_program() {
        let table = sample_table();
        let file = cmtrace_like(&table);
        let found = cmtrace_messages(&file).expect("the embedded table is found");
        assert_eq!(found, &table[..]);
        assert_eq!(
            find_message(found, 0x87D0_0215).as_deref(),
            Some("Item not found\r\n")
        );
    }

    #[test]
    fn names_match_without_regard_to_case() {
        let table = sample_table();
        let program = with_table(&table);
        let file = pe_file(&[(Key::Name("binres"), &[(Key::Name("Trace32"), &program)])]);
        assert_eq!(cmtrace_messages(&file), Some(&table[..]));
    }

    #[test]
    fn files_without_a_table_have_none() {
        assert_eq!(cmtrace_messages(b""), None);
        assert_eq!(cmtrace_messages(b"MZ"), None);
        assert_eq!(cmtrace_messages(&[0u8; 4096]), None);
        let file = pe_file(&[(Key::Number(3), &[(Key::Number(1), b"an icon")])]);
        assert_eq!(cmtrace_messages(&file), None);
        // A program resource that isn't a program.
        let file = pe_file(&[(Key::Name("BINRES"), &[(Key::Name("TRACE64"), b"MZ...")])]);
        assert_eq!(cmtrace_messages(&file), None);
    }

    #[test]
    fn a_cut_short_file_never_panics() {
        let file = cmtrace_like(&sample_table());
        for length in 0..file.len() {
            let _ = cmtrace_messages(&file[..length]);
        }
    }

    proptest! {
        #[test]
        fn any_bytes_are_handled(bytes in proptest::collection::vec(any::<u8>(), 0..2048), code in any::<u32>()) {
            let _ = cmtrace_messages(&bytes);
            let _ = find_message(&bytes, code);
        }

        #[test]
        fn a_damaged_file_is_handled(
            changes in proptest::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..16),
            code in 0x87D0_0210u32..0x87D0_0218,
        ) {
            let mut file = cmtrace_like(&sample_table());
            for (index, byte) in changes {
                let at = index.index(file.len());
                file[at] = byte;
            }
            if let Some(table) = cmtrace_messages(&file) {
                let _ = find_message(table, code);
            }
        }
    }
}
