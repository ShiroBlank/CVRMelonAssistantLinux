//! melon_dll.rs — Extract MelonInfoAttribute and assembly dependencies from
//! MelonLoader mod DLLs using a lightweight PE / CLI metadata parser.

use std::path::Path;
use std::fs;

// ── Known non-mod assembly names to filter out of AssemblyRef deps ────────────
// These are framework, Unity, and MelonLoader-internal assemblies that are never
// CVRMG mods and should never trigger auto-install.
static SKIP_ASSEMBLIES: &[&str] = &[
    "mscorlib", "netstandard", "System", "System.Core", "System.Xml",
    "System.Runtime", "System.Collections", "System.Linq", "System.Text",
    "System.IO", "System.Reflection", "System.Threading", "System.Memory",
    "System.Buffers", "System.Numerics", "System.ComponentModel",
    "System.Diagnostics", "System.Net", "System.Security",
    "Microsoft", "Windows",
    "UnityEngine", "Unity",
    "Assembly-CSharp", "Assembly-CSharp-firstpass",
    "MelonLoader", "0Harmony", "Harmony", "MonoMod",
    "Il2Cppmscorlib", "Il2CppSystem", "Il2CppInterop",
    "Newtonsoft.Json", "HarmonyXInterop",
];

fn should_skip(name: &str) -> bool {
    let lower = name.to_lowercase();
    SKIP_ASSEMBLIES.iter().any(|skip| {
        let s = skip.to_lowercase();
        lower == s || lower.starts_with(&format!("{}.", s)) || lower.starts_with(&format!("{},", s))
    })
}

// ── Public structs ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MelonInfo {
    pub name:    String,
    pub version: String,
    pub author:  String,
}

/// Extract MelonInfoAttribute from a mod DLL file.
pub fn read_melon_info(path: &Path) -> Option<MelonInfo> {
    let bytes = fs::read(path).ok()?;
    extract_melon_info(&bytes)
}

/// Extract MelonInfoAttribute from raw DLL bytes.
pub fn extract_melon_info(data: &[u8]) -> Option<MelonInfo> {
    let ctx = MetaContext::parse(data)?;

    // Confirm this is a Melon mod
    let has_melon = ctx.strings_data.windows(9).any(|w| w == b"MelonInfo");
    if !has_melon { return None; }

    scan_blob_for_melon_info(ctx.blobs)
}

/// Extract non-system assembly dependencies from the AssemblyRef table.
/// These are the hard dependencies that must be present for the mod to load.
pub fn extract_additional_deps(data: &[u8]) -> Vec<String> {
    extract_assembly_refs(data)
        .into_iter()
        .filter(|name| !should_skip(name))
        .collect()
}

// ── CLI metadata context ──────────────────────────────────────────────────────

struct MetaContext<'a> {
    strings_data: &'a [u8],
    blobs:        &'a [u8],
    tables:       &'a [u8],
    strings_wide: bool,
    blob_wide:    bool,
    _guid_wide:   bool,
    row_counts:   [u32; 64],
    table_data_start: usize,
}

impl<'a> MetaContext<'a> {
    fn parse(data: &'a [u8]) -> Option<Self> {
        let cli_offset = find_cli_metadata(data)?;
        let meta = &data[cli_offset..];
        if meta.len() < 20 { return None; }
        if read_u32(meta, 0)? != 0x424A5342 { return None; }

        let version_len     = read_u32(meta, 12)? as usize;
        let aligned         = (version_len + 3) & !3;
        let streams_base    = 16 + aligned;
        if streams_base + 4 > meta.len() { return None; }
        let stream_count    = read_u16(meta, streams_base + 2)? as usize;
        let mut sh_off      = streams_base + 4;

        let mut strings_off  = None; let mut strings_sz  = 0usize;
        let mut blob_off     = None; let mut blob_sz      = 0usize;
        let mut tables_off   = None; let mut tables_sz    = 0usize;

        for _ in 0..stream_count {
            if sh_off + 8 > meta.len() { break; }
            let offset = read_u32(meta, sh_off)? as usize;
            let size   = read_u32(meta, sh_off + 4)? as usize;
            let name   = read_cstr(meta, sh_off + 8)?;
            let padded = (name.len() + 1 + 3) & !3;
            sh_off     = sh_off + 8 + padded;
            let abs    = cli_offset + offset;
            match name.as_str() {
                "#Strings" => { strings_off = Some(abs); strings_sz = size; }
                "#Blob"    => { blob_off    = Some(abs); blob_sz    = size; }
                "#~" | "#-" => { tables_off = Some(abs); tables_sz = size; }
                _ => {}
            }
        }

        let strings_off  = strings_off?;
        let blob_off     = blob_off?;
        let tables_off   = tables_off?;

        if strings_off + strings_sz > data.len()
            || blob_off + blob_sz > data.len()
            || tables_off + tables_sz > data.len()
        { return None; }

        let strings_data = &data[strings_off..strings_off + strings_sz];
        let blobs        = &data[blob_off..blob_off + blob_sz];
        let tables       = &data[tables_off..tables_off + tables_sz];

        if tables.len() < 24 { return None; }
        let heap_sizes  = tables[6];
        let strings_wide = (heap_sizes & 0x01) != 0;
        let blob_wide    = (heap_sizes & 0x04) != 0;
        let guid_wide    = (heap_sizes & 0x02) != 0;

        let valid_mask   = read_u64(tables, 8)?;
        let mut row_counts = [0u32; 64];
        let mut pos = 24usize;
        for i in 0..64usize {
            if (valid_mask >> i) & 1 == 1 {
                if pos + 4 > tables.len() { return None; }
                row_counts[i] = read_u32(tables, pos)?;
                pos += 4;
            }
        }

        Some(MetaContext {
            strings_data, blobs, tables,
            strings_wide, blob_wide, _guid_wide: guid_wide,
            row_counts,
            table_data_start: pos,
        })
    }

    fn s_idx(&self) -> usize { if self.strings_wide { 4 } else { 2 } }
    fn b_idx(&self) -> usize { if self.blob_wide    { 4 } else { 2 } }
    fn g_idx(&self) -> usize { if self._guid_wide   { 4 } else { 2 } }

    fn simple_idx(&self, table: usize) -> usize {
        if self.row_counts.get(table).copied().unwrap_or(0) > 0xFFFF { 4 } else { 2 }
    }

    fn coded_idx(&self, tables: &[usize], tag_bits: u32) -> usize {
        let max_rows = tables.iter()
            .map(|&t| self.row_counts.get(t).copied().unwrap_or(0) as usize)
            .max()
            .unwrap_or(0);
        let limit = 1usize << (16 - tag_bits);
        if max_rows >= limit { 4 } else { 2 }
    }

    /// Compute the byte size of a single row for the given table index.
    fn row_size(&self, t: usize) -> usize {
        let s = self.s_idx();
        let b = self.b_idx();
        let g = self.g_idx();
        let u2 = 2usize; let u4 = 4usize;

        // Coded index helpers
        let type_deforef   = self.coded_idx(&[0x01,0x02,0x1B], 2);
        let has_ca         = self.coded_idx(&(0..=0x27usize).collect::<Vec<_>>().as_slice(), 5);
        let ca_type        = self.coded_idx(&[0x06, 0x0A], 3);
        let has_const      = self.coded_idx(&[0x04, 0x08, 0x17], 2);
        let has_field_m    = self.coded_idx(&[0x04, 0x08, 0x17], 2);
        let has_decl       = self.coded_idx(&[0x01,0x02,0x06,0x08,0x10,0x11,0x14,0x17,0x20,0x23], 2);
        let member_ref_p   = self.coded_idx(&[0x00,0x01,0x02,0x1A,0x1B], 3);
        let meth_deforef   = self.coded_idx(&[0x06, 0x0A], 1);
        let member_fwd     = self.coded_idx(&[0x04, 0x06], 1);
        let has_sem        = self.coded_idx(&[0x14, 0x17], 1);
        let resolution_sc  = self.coded_idx(&[0x00,0x01,0x1A,0x23], 2);

        match t {
            0x00 => u2+s+g+g+g,
            0x01 => resolution_sc+s+s,
            0x02 => u4+s+s+type_deforef+self.simple_idx(0x04)+self.simple_idx(0x06),
            0x03 => self.simple_idx(0x04),
            0x04 => u2+s+b,
            0x05 => self.simple_idx(0x06),
            0x06 => u4+u2+u2+s+b+self.simple_idx(0x08),
            0x07 => self.simple_idx(0x08),
            0x08 => u2+u2+s,
            0x09 => self.simple_idx(0x02)+type_deforef,
            0x0A => member_ref_p+s+b,
            0x0B => u2+has_const+b,
            0x0C => has_ca+ca_type+b,
            0x0D => has_field_m+b,
            0x0E => u2+has_decl+b,
            0x0F => u2+u4+self.simple_idx(0x02),
            0x10 => u4+self.simple_idx(0x04),
            0x11 => b,
            0x12 => self.simple_idx(0x02)+self.simple_idx(0x14),
            0x13 => self.simple_idx(0x14),
            0x14 => u2+s+type_deforef,
            0x15 => self.simple_idx(0x02)+self.simple_idx(0x17),
            0x16 => self.simple_idx(0x17),
            0x17 => u2+s+b,
            0x18 => u2+self.simple_idx(0x06)+has_sem,
            0x19 => self.simple_idx(0x02)+meth_deforef+meth_deforef,
            0x1A => s,
            0x1B => b,
            0x1C => u2+member_fwd+s+self.simple_idx(0x1A),
            0x1D => u4+self.simple_idx(0x04),
            0x1E => u4+u4,
            0x1F => u4,
            0x20 => u4+u2+u2+u2+u2+u4+b+s+s,
            0x21 => u4,
            0x22 => u4+u4+u4,
            // AssemblyRef (0x23): u16*4 + u32 + b_idx + s_idx + s_idx + b_idx
            0x23 => u2+u2+u2+u2+u4+b+s+s+b,
            _ => 0,
        }
    }

    /// Read a string-heap index (2 or 4 bytes) and return the string.
    fn read_string_at(&self, table_bytes: &[u8], offset: usize) -> Option<String> {
        let idx = if self.strings_wide {
            read_u32(table_bytes, offset)? as usize
        } else {
            read_u16(table_bytes, offset)? as usize
        };
        if idx >= self.strings_data.len() { return None; }
        let end = self.strings_data[idx..].iter().position(|&b| b == 0)
            .map(|p| idx + p)
            .unwrap_or(self.strings_data.len());
        Some(String::from_utf8_lossy(&self.strings_data[idx..end]).to_string())
    }
}

// ── AssemblyRef reader ────────────────────────────────────────────────────────

fn extract_assembly_refs(data: &[u8]) -> Vec<String> {
    let ctx = match MetaContext::parse(data) {
        Some(c) => c,
        None    => return vec![],
    };

    let asmref_count = ctx.row_counts.get(0x23).copied().unwrap_or(0) as usize;
    if asmref_count == 0 { return vec![]; }

    // Compute byte offset of AssemblyRef table by summing preceding table sizes
    let mut byte_offset = ctx.table_data_start;
    for t in 0x00usize..0x23 {
        let rows = ctx.row_counts.get(t).copied().unwrap_or(0) as usize;
        if rows > 0 {
            let rs = ctx.row_size(t);
            if rs == 0 { return vec![]; }  // unknown table — bail out safely
            byte_offset += rows * rs;
        }
    }

    let asmref_row_size = ctx.row_size(0x23);
    if asmref_row_size == 0 { return vec![]; }

    // Name column offset within an AssemblyRef row:
    //   0: MajorVersion   (u16)
    //   2: MinorVersion   (u16)
    //   4: BuildNumber    (u16)
    //   6: RevisionNumber (u16)
    //   8: Flags          (u32)
    //  12: PublicKeyOrToken (blob_idx)
    //  12+b: Name          (string_idx)  ← this is what we want
    let name_col_offset = 12 + ctx.b_idx();

    let mut deps = Vec::new();
    for i in 0..asmref_count {
        let row_off = byte_offset + i * asmref_row_size;
        if row_off + name_col_offset + ctx.s_idx() > ctx.tables.len() { break; }
        if let Some(name) = ctx.read_string_at(ctx.tables, row_off + name_col_offset) {
            if !name.is_empty() {
                deps.push(name);
            }
        }
    }
    deps
}

// ── MelonInfo blob scanner (unchanged) ───────────────────────────────────────

fn scan_blob_for_melon_info(blobs: &[u8]) -> Option<MelonInfo> {
    let mut i = 0;
    while i + 4 < blobs.len() {
        let (blob_data_len, prefix_bytes) = read_compressed_uint(blobs, i);
        if blob_data_len == 0 || prefix_bytes == 0 { i += 1; continue; }
        let data_start = i + prefix_bytes;
        let data_end   = data_start + blob_data_len;
        if data_end > blobs.len() { i += 1; continue; }
        let blob = &blobs[data_start..data_end];
        if blob.len() >= 6 && blob[0] == 0x01 && blob[1] == 0x00 {
            if let Some(info) = try_parse_melon_blob(blob) {
                return Some(info);
            }
        }
        i = data_end;
    }
    None
}

fn try_parse_melon_blob(blob: &[u8]) -> Option<MelonInfo> {
    let mut pos = 2usize;
    let (_, skip) = try_read_string(blob, pos)?;
    pos += skip;
    if pos >= blob.len() { return None; }
    let (name, skip) = try_read_string(blob, pos)?;
    pos += skip;
    if pos >= blob.len() { return None; }
    let (version, skip) = try_read_string(blob, pos)?;
    pos += skip;
    let author = if pos < blob.len() {
        if let Some((a, _)) = try_read_string(blob, pos) { a } else { String::new() }
    } else { String::new() };

    if name.is_empty() || name.len() > 128 { return None; }
    if version.is_empty() || version.len() > 32 { return None; }
    if !version.chars().any(|c| c.is_ascii_digit()) { return None; }
    if !name.chars().all(|c| c >= ' ' && c != '\0') { return None; }
    Some(MelonInfo { name, version, author })
}

// ── Low-level helpers ─────────────────────────────────────────────────────────

fn try_read_string(data: &[u8], pos: usize) -> Option<(String, usize)> {
    if pos >= data.len() { return None; }
    if data[pos] == 0xFF { return Some((String::new(), 1)); }
    let (len, prefix) = read_compressed_uint(data, pos);
    if prefix == 0 { return None; }
    let str_start = pos + prefix;
    let str_end   = str_start + len;
    if str_end > data.len() { return None; }
    let s = String::from_utf8_lossy(&data[str_start..str_end]).to_string();
    Some((s, prefix + len))
}

fn read_compressed_uint(data: &[u8], pos: usize) -> (usize, usize) {
    if pos >= data.len() { return (0, 0); }
    let b0 = data[pos] as usize;
    if b0 & 0x80 == 0 { return (b0, 1); }
    else if b0 & 0xC0 == 0x80 {
        if pos + 1 >= data.len() { return (0, 0); }
        return (((b0 & 0x3F) << 8) | data[pos+1] as usize, 2);
    } else if b0 & 0xE0 == 0xC0 {
        if pos + 3 >= data.len() { return (0, 0); }
        let b1 = data[pos+1] as usize; let b2 = data[pos+2] as usize; let b3 = data[pos+3] as usize;
        return (((b0 & 0x1F) << 24) | (b1 << 16) | (b2 << 8) | b3, 4);
    }
    (0, 0)
}

fn find_cli_metadata(data: &[u8]) -> Option<usize> {
    if data.len() < 64 { return None; }
    if data[0] != 0x4D || data[1] != 0x5A { return None; }
    let pe_offset = read_u32(data, 60)? as usize;
    if pe_offset + 4 > data.len() { return None; }
    if &data[pe_offset..pe_offset + 4] != b"PE\0\0" { return None; }
    let optional_header_offset = pe_offset + 4 + 20;
    if optional_header_offset + 2 > data.len() { return None; }
    let magic = read_u16(data, optional_header_offset)?;
    let clr_data_dir_offset = match magic {
        0x010B => optional_header_offset + 96 + 14 * 8,
        0x020B => optional_header_offset + 112 + 14 * 8,
        _      => return None,
    };
    if clr_data_dir_offset + 8 > data.len() { return None; }
    let clr_rva = read_u32(data, clr_data_dir_offset)? as usize;
    if clr_rva == 0 { return None; }
    let clr_file_offset = rva_to_file_offset(data, pe_offset, clr_rva)?;
    if clr_file_offset + 72 > data.len() { return None; }
    let meta_rva = read_u32(data, clr_file_offset + 8)? as usize;
    if meta_rva == 0 { return None; }
    rva_to_file_offset(data, pe_offset, meta_rva)
}

fn rva_to_file_offset(data: &[u8], pe_offset: usize, rva: usize) -> Option<usize> {
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
    Some(u32::from_le_bytes([data[offset], data[offset+1], data[offset+2], data[offset+3]]))
}
fn read_u64(data: &[u8], offset: usize) -> Option<u64> {
    if offset + 8 > data.len() { return None; }
    Some(u64::from_le_bytes(data[offset..offset+8].try_into().ok()?))
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
