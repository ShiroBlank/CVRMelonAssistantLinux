// melon_dll.rs — Extract MelonInfoAttribute from a MelonLoader mod DLL.
//
// MelonLoader mods embed an assembly-level attribute like:
//   [assembly: MelonInfo(typeof(MyMod), "My Mod Name", "1.2.3", "AuthorName")]
// or the older:
//   [assembly: MelonModInfo(typeof(MyMod), "My Mod Name", "1.2.3", "AuthorName")]
//
// These get compiled into the .NET PE as a CustomAttribute on the assembly.
// The attribute constructor args are stored as a blob in the #Blob heap,
// preceded by the string heap entries for each string argument.
//
// We implement a lightweight PE / CLI metadata parser — no external crate needed —
// that locates the #Strings and #Blob heaps, finds the attribute row, and reads
// the constructor arguments.

use std::path::Path;
use std::fs;

#[derive(Debug, Clone)]
pub struct MelonInfo {
    pub name:    String,
    pub version: String,
    pub author:  String,
}

/// Read MelonInfo from a .dll. Returns None if the file can't be parsed or
/// doesn't contain a MelonInfoAttribute.
pub fn read_melon_info(path: &Path) -> Option<MelonInfo> {
    let bytes = fs::read(path).ok()?;
    extract_melon_info(&bytes)
}

fn extract_melon_info(data: &[u8]) -> Option<MelonInfo> {
    // ── Step 1: locate the CLI metadata root via the PE header ───────────────
    let cli_offset = find_cli_metadata(data)?;
    let meta = &data[cli_offset..];

    // ── Step 2: parse the CLI metadata root ───────────────────────────────────
    // Signature: 0x424A5342 ("BSJB")
    if meta.len() < 20 { return None; }
    if read_u32(meta, 0)? != 0x424A5342 { return None; }

    let version_len = read_u32(meta, 12)? as usize;
    let aligned_version_len = (version_len + 3) & !3;
    let streams_offset = 16 + aligned_version_len;
    if streams_offset + 4 > meta.len() { return None; }

    let stream_count = read_u16(meta, streams_offset + 2)? as usize;
    let mut stream_headers_offset = streams_offset + 4;

    let mut strings_offset: Option<usize> = None;
    let mut strings_size:   Option<usize> = None;
    let mut blob_offset:    Option<usize> = None;
    let mut blob_size:      Option<usize> = None;
    let mut tables_offset:  Option<usize> = None;

    for _ in 0..stream_count {
        if stream_headers_offset + 8 > meta.len() { break; }
        let offset = read_u32(meta, stream_headers_offset)? as usize;
        let size   = read_u32(meta, stream_headers_offset + 4)? as usize;

        // Stream name follows as null-terminated string, padded to 4 bytes
        let name_start = stream_headers_offset + 8;
        let name = read_cstr(meta, name_start)?;
        let name_padded = (name.len() + 1 + 3) & !3;
        stream_headers_offset = name_start + name_padded;

        match name.as_str() {
            "#Strings" => { strings_offset = Some(cli_offset + offset); strings_size = Some(size); }
            "#Blob"    => { blob_offset    = Some(cli_offset + offset); blob_size    = Some(size); }
            "#~" | "#-" => { tables_offset = Some(cli_offset + offset); }
            _ => {}
        }
    }

    let strings_offset = strings_offset?;
    let blob_offset    = blob_offset?;
    let tables_offset  = tables_offset?;

    // ── Step 3: parse the metadata tables header ──────────────────────────────
    // Tables stream: 6-byte header, then 64-bit valid mask, 64-bit sorted mask,
    // then row counts for each set bit.
    let tables = &data[tables_offset..];
    if tables.len() < 24 { return None; }

    let heap_sizes  = tables[6]; // bit 0 = strings wide, bit 1 = guid wide, bit 2 = blob wide
    let valid_mask  = read_u64(tables, 8)?;

    let strings_wide = (heap_sizes & 0x01) != 0;
    let blob_wide    = (heap_sizes & 0x04) != 0;

    // Count rows for each table present before the ones we care about
    let mut row_counts = [0u32; 64];
    let mut pos = 24usize;
    for i in 0..64usize {
        if (valid_mask >> i) & 1 == 1 {
            if pos + 4 > tables.len() { return None; }
            row_counts[i] = read_u32(tables, pos)?;
            pos += 4;
        }
    }

    // Table indices we need:
    //   0x01 = TypeRef, 0x0A = MemberRef, 0x20 = Assembly, 0x24 = CustomAttribute
    const TABLE_TYPE_REF:        usize = 0x01;
    const TABLE_MEMBER_REF:      usize = 0x0A;
    const TABLE_CUSTOM_ATTRIBUTE:usize = 0x0C;

    // We need to compute the byte offset of each table. Tables appear in
    // ascending index order for all set bits in valid_mask.
    // Row sizes vary based on heap widths and cross-table reference sizes.

    // First compute reference widths (1 or 2 bytes each depending on row count)
    let _type_ref_rows   = row_counts[TABLE_TYPE_REF]   as usize;
    let _member_ref_rows = row_counts[TABLE_MEMBER_REF] as usize;
    let ca_rows         = row_counts[TABLE_CUSTOM_ATTRIBUTE] as usize;

    if ca_rows == 0 { return None; }

    // Coded index widths — we only need the ones used by CustomAttribute
    // HasCustomAttribute: 22 tables, max tag = 5 bits → row limit for 2-byte = (2^11 - 1) = 2047
    // CustomAttributeType: 5 choices, tag = 3 bits → row limit for 2-byte = (2^13 - 1) = 8191

    let _str_idx_size  = if strings_wide { 4usize } else { 2 };
    let _blob_idx_size = if blob_wide    { 4usize } else { 2 };

    // CustomAttribute row layout:
    //   Parent  (HasCustomAttribute coded index)
    //   Type    (CustomAttributeType coded index)
    //   Value   (blob index)

    // We need to skip to the CustomAttribute table, then scan its rows looking
    // for an attribute whose Type resolves to "MelonInfoAttribute" or "MelonModInfoAttribute".

    // To avoid implementing the full metadata table layout for every preceding
    // table, we use a simpler approach: scan the entire #Strings heap for the
    // attribute names, then scan the blob heap for the value blob.

    // ── Shortcut: scan #Strings heap for the attribute type name ─────────────
    // Once we find it, we know roughly where attribute values live in #Blob.

    // Actually the cleanest approach that avoids full table parsing:
    // Scan the raw #Blob heap for the CustomAttribute value blob format.
    // CustomAttribute value blobs start with 0x0001 (prolog), then serialised
    // constructor arguments. For string args: a compressed length followed by UTF-8.
    // We look for 0x00 0x01 followed by string data that looks like
    // a mod name, version ("x.y.z"), and author.

    let blobs = &data[blob_offset..blob_offset + blob_size.unwrap_or(0)];
    let strings_data = &data[strings_offset..strings_offset + strings_size.unwrap_or(0)];

    // ── Step 4: find "MelonInfoAttribute" in strings heap → confirms this is a melon mod
    let has_melon_attr = strings_data.windows(9).any(|w| w == b"MelonInfo")
        || strings_data.windows(12).any(|w| w == b"MelonModInfo");

    if !has_melon_attr {
        return None;
    }

    // ── Step 5: scan the blob heap for a CustomAttribute value blob that has
    //    the right shape: prolog 0x01 0x00, then 4 serialized args where
    //    args 2,3,4 (name, version, author) are non-empty strings.
    scan_blob_for_melon_info(blobs)
}

/// Scan a blob heap for a CustomAttribute value blob that looks like
/// MelonInfoAttribute(Type, name, version, author, ...).
/// The blob format is: 01 00 <args> 00 00 (num_named_args u16 LE at end)
fn scan_blob_for_melon_info(blobs: &[u8]) -> Option<MelonInfo> {
    let mut i = 0;
    while i + 4 < blobs.len() {
        // Skip blob length prefix (compressed uint)
        let (blob_data_len, prefix_bytes) = read_compressed_uint(blobs, i);
        if blob_data_len == 0 || prefix_bytes == 0 {
            i += 1;
            continue;
        }
        let data_start = i + prefix_bytes;
        let data_end = data_start + blob_data_len;
        if data_end > blobs.len() {
            i += 1;
            continue;
        }

        let blob = &blobs[data_start..data_end];

        // CustomAttribute value blob prolog: 01 00
        if blob.len() >= 6 && blob[0] == 0x01 && blob[1] == 0x00 {
            if let Some(info) = try_parse_melon_blob(blob) {
                return Some(info);
            }
        }

        i = data_end;
    }
    None
}

/// Try to parse a blob as MelonInfoAttribute(Type type, string name, string version, string author)
/// After the 0x01 0x00 prolog, the first arg is a System.Type (serialized as a string of the type name).
/// Args 2-4 are the name, version, author strings.
fn try_parse_melon_blob(blob: &[u8]) -> Option<MelonInfo> {
    // Skip prolog (2 bytes)
    let mut pos = 2usize;

    // Arg 1: Type — serialized as a string (type name). Read and skip it.
    let (_, skip) = try_read_string(blob, pos)?;
    pos += skip;
    if pos >= blob.len() { return None; }

    // Arg 2: name
    let (name, skip) = try_read_string(blob, pos)?;
    pos += skip;
    if pos >= blob.len() { return None; }

    // Arg 3: version
    let (version, skip) = try_read_string(blob, pos)?;
    pos += skip;

    // Arg 4: author (may not be present in all versions)
    let author = if pos < blob.len() {
        if let Some((a, _)) = try_read_string(blob, pos) { a } else { String::new() }
    } else {
        String::new()
    };

    // Validate: name and version should look plausible
    if name.is_empty() || name.len() > 128 { return None; }
    if version.is_empty() || version.len() > 32 { return None; }
    // Version should contain at least one digit
    if !version.chars().any(|c| c.is_ascii_digit()) { return None; }
    // Name should be printable ASCII / UTF-8
    if !name.chars().all(|c| c >= ' ' && c != '\0') { return None; }

    Some(MelonInfo { name, version, author })
}

/// Read a .NET metadata compressed-length-prefixed UTF-8 string.
/// Returns (string, bytes_consumed) or None.
fn try_read_string(data: &[u8], pos: usize) -> Option<(String, usize)> {
    if pos >= data.len() { return None; }
    // 0xFF means null string
    if data[pos] == 0xFF { return Some((String::new(), 1)); }

    let (len, prefix) = read_compressed_uint(data, pos);
    if prefix == 0 { return None; }
    let str_start = pos + prefix;
    let str_end   = str_start + len;
    if str_end > data.len() { return None; }

    let s = String::from_utf8_lossy(&data[str_start..str_end]).to_string();
    Some((s, prefix + len))
}

/// Decode a .NET compressed unsigned integer (1, 2, or 4 bytes).
/// Returns (value, bytes_consumed). Returns (0, 0) on error.
fn read_compressed_uint(data: &[u8], pos: usize) -> (usize, usize) {
    if pos >= data.len() { return (0, 0); }
    let b0 = data[pos] as usize;
    if b0 & 0x80 == 0 {
        (b0, 1)
    } else if b0 & 0xC0 == 0x80 {
        if pos + 1 >= data.len() { return (0, 0); }
        let b1 = data[pos + 1] as usize;
        (((b0 & 0x3F) << 8) | b1, 2)
    } else if b0 & 0xE0 == 0xC0 {
        if pos + 3 >= data.len() { return (0, 0); }
        let b1 = data[pos + 1] as usize;
        let b2 = data[pos + 2] as usize;
        let b3 = data[pos + 3] as usize;
        (((b0 & 0x1F) << 24) | (b1 << 16) | (b2 << 8) | b3, 4)
    } else {
        (0, 0)
    }
}

/// Find the offset of the CLI metadata root in a PE file.
fn find_cli_metadata(data: &[u8]) -> Option<usize> {
    // DOS header: e_magic = 0x5A4D, e_lfanew at offset 60
    if data.len() < 64 { return None; }
    if data[0] != 0x4D || data[1] != 0x5A { return None; } // "MZ"

    let pe_offset = read_u32(data, 60)? as usize;
    if pe_offset + 4 > data.len() { return None; }
    if &data[pe_offset..pe_offset + 4] != b"PE\0\0" { return None; }

    // COFF header: 20 bytes starting at pe_offset + 4
    let optional_header_offset = pe_offset + 4 + 20;
    if optional_header_offset + 2 > data.len() { return None; }

    let magic = read_u16(data, optional_header_offset)?;
    let (clr_data_dir_offset, _rva_to_section_helper) = match magic {
        0x010B => { // PE32
            // CLR runtime header is data directory entry 14 (index from 0)
            // Data directories start at optional_header_offset + 96
            (optional_header_offset + 96 + 14 * 8, optional_header_offset)
        }
        0x020B => { // PE32+ (64-bit)
            // Data directories start at optional_header_offset + 112
            (optional_header_offset + 112 + 14 * 8, optional_header_offset)
        }
        _ => return None,
    };

    if clr_data_dir_offset + 8 > data.len() { return None; }
    let clr_rva  = read_u32(data, clr_data_dir_offset)? as usize;
    if clr_rva == 0 { return None; }

    // Resolve the CLR header RVA to a file offset via the section table
    let clr_file_offset = rva_to_file_offset(data, pe_offset, clr_rva)?;
    if clr_file_offset + 72 > data.len() { return None; }

    // CLR header: MetaData RVA is at offset 8, MetaData size at offset 12
    let meta_rva = read_u32(data, clr_file_offset + 8)? as usize;
    if meta_rva == 0 { return None; }

    rva_to_file_offset(data, pe_offset, meta_rva)
}

/// Resolve a Relative Virtual Address to a file offset using the section table.
fn rva_to_file_offset(data: &[u8], pe_offset: usize, rva: usize) -> Option<usize> {
    // COFF header at pe_offset + 4: NumberOfSections at offset 2
    let coff = pe_offset + 4;
    if coff + 20 > data.len() { return None; }
    let num_sections = read_u16(data, coff + 2)? as usize;
    let optional_size = read_u16(data, coff + 16)? as usize;
    let section_table_offset = coff + 20 + optional_size;

    for i in 0..num_sections {
        let sec = section_table_offset + i * 40;
        if sec + 40 > data.len() { break; }
        let virtual_size    = read_u32(data, sec + 8)?  as usize;
        let virtual_address = read_u32(data, sec + 12)? as usize;
        let raw_offset      = read_u32(data, sec + 20)? as usize;

        if rva >= virtual_address && rva < virtual_address + virtual_size {
            return Some(raw_offset + (rva - virtual_address));
        }
    }
    None
}

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    if offset + 2 > data.len() { return None; }
    Some(u16::from_le_bytes([data[offset], data[offset + 1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    if offset + 4 > data.len() { return None; }
    Some(u32::from_le_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]]))
}

fn read_u64(data: &[u8], offset: usize) -> Option<u64> {
    if offset + 8 > data.len() { return None; }
    Some(u64::from_le_bytes(data[offset..offset + 8].try_into().ok()?))
}

fn read_cstr(data: &[u8], offset: usize) -> Option<String> {
    let end = data[offset..].iter().position(|&b| b == 0)?;
    Some(String::from_utf8_lossy(&data[offset..offset + end]).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compressed_uint() {
        assert_eq!(read_compressed_uint(&[0x03], 0), (3, 1));
        assert_eq!(read_compressed_uint(&[0x81, 0x23], 0), (0x123, 2));
    }
}
