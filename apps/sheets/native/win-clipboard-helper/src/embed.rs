//! OLE embedding bytes for the clipboard (genoffice#1985 stage 2). The
//! layouts here follow [MS-OLEDS] v20240423:
//!
//! - `Embed Source` (registered clipboard format) carries a complete OLE 2
//!   Compound File whose streams are `\1Ole` (OLEStream, section 2.3.3),
//!   `\1CompObj` (CompObjStream, 2.3.8), `\1Ole10Native` (OLENativeStream,
//!   2.3.6 — the workbook's own bytes as native data) and `\3OlePres000`
//!   (OLEPresentationStream, 2.3.4 — the EMF the container displays). A
//!   container pastes this via StgOpenStorage + OleLoad, exactly as it would
//!   for an Excel copy.
//! - `Object Descriptor` (registered format) carries the OBJECTDESCRIPTOR
//!   structure so paste dialogs can show the object type before pasting.
//!
//! Everything here is pure byte assembly, compiled and tested on every
//! platform; only the clipboard handoff is Windows-specific.

use base64::Engine;
use std::io::{Cursor, Read, Write};

/// The embedded object's class GUID. Deliberately GenOffice's own, not a
/// borrowed Excel class: containers resolve this CLSID for activation, and
/// until an OLE server is registered for it, double-click reports the class
/// as unregistered instead of hijacking Excel (genoffice#1985 stage 3).
pub const GENOFFICE_SHEET_CLSID: uuid::Uuid =
    uuid::Uuid::from_u128(0x8C10_B647_B13C_4511_8B4F_2E31_E5A9_D7C6);

/// DVASPECT_CONTENT (the presentation shows the whole object).
const ASPECT_CONTENT: u32 = 1;
/// ClipboardFormatOrAnsiString marker: FormatOrAnsiString is a standard format id (2.3.1).
const CF_MARKER_STANDARD: u32 = 0xFFFF_FFFE;
/// CF_ENHMETAFILE (MS-OLEDS 2.1.1).
const CF_ENHMETAFILE: u32 = 0x0000_000E;
/// UnicodeMarker that must precede the Unicode half of a CompObjStream (2.3.8).
const COMPOBJ_UNICODE_MARKER: u32 = 0x71B2_39F4;
/// OlePres TOC signature; TocCount 0 means no entries follow (2.3.4).
const OLEPRES_TOC_SIGNATURE: u32 = 0x494E_414E;

/// LengthPrefixedAnsiString (2.1.4): u32 char count including the null, then
/// the null-terminated bytes. Length 0 encodes an empty string with no bytes.
fn lp_ansi(value: &str) -> Vec<u8> {
    let mut out = Vec::new();
    if value.is_empty() {
        out.extend_from_slice(&0u32.to_le_bytes());
        return out;
    }
    out.extend_from_slice(&((value.len() + 1) as u32).to_le_bytes());
    out.extend_from_slice(value.as_bytes());
    out.push(0);
    out
}

/// LengthPrefixedUnicodeString (2.1.5): u32 byte count including the null,
/// then the UTF-16LE bytes with the terminating null.
fn lp_unicode(value: &str) -> Vec<u8> {
    let mut out = Vec::new();
    if value.is_empty() {
        out.extend_from_slice(&0u32.to_le_bytes());
        return out;
    }
    let units: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    out.extend_from_slice(&((units.len() * 2) as u32).to_le_bytes());
    for unit in units {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

/// `\1Ole` — the OLEStream structure for an embedded object (2.3.3): version,
/// embedded flag, no monikers. 20 bytes; the fields after
/// ReservedMonikerStreamSize MUST NOT be present for embedded objects.
pub fn build_ole_stream() -> Vec<u8> {
    let mut out = Vec::with_capacity(20);
    out.extend_from_slice(&0x0200_0001u32.to_le_bytes()); // Version
    out.extend_from_slice(&0u32.to_le_bytes()); // Flags: embedded
    out.extend_from_slice(&0u32.to_le_bytes()); // LinkUpdateOption
    out.extend_from_slice(&0u32.to_le_bytes()); // Reserved1
    out.extend_from_slice(&0u32.to_le_bytes()); // ReservedMonikerStreamSize
    out
}

/// `\1CompObj` — CompObjStream (2.3.8): header, ANSI user type and clipboard
/// format, then the Unicode half. The clipboard format names the presentation
/// data the container can ask the object for (CF_ENHMETAFILE here).
pub fn build_comp_obj(user_type: &str) -> Vec<u8> {
    let mut out = Vec::new();
    // CompObjHeader (2.3.7): Reserved1, Version, Reserved2[20]. The reserved
    // values are arbitrary per spec; these match the v3 layout common tools
    // write (0xFFFE0001, 3).
    out.extend_from_slice(&0xFFFE_0001u32.to_le_bytes());
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 20]);
    out.extend_from_slice(&lp_ansi(user_type)); // AnsiUserType
    out.extend_from_slice(&CF_MARKER_STANDARD.to_le_bytes());
    out.extend_from_slice(&CF_ENHMETAFILE.to_le_bytes()); // AnsiClipboardFormat
    out.extend_from_slice(&lp_ansi("")); // Reserved1
    out.extend_from_slice(&COMPOBJ_UNICODE_MARKER.to_le_bytes()); // UnicodeMarker
    out.extend_from_slice(&lp_unicode(user_type)); // UnicodeUserType
    out.extend_from_slice(&CF_MARKER_STANDARD.to_le_bytes());
    out.extend_from_slice(&CF_ENHMETAFILE.to_le_bytes()); // UnicodeClipboardFormat
    out.extend_from_slice(&lp_unicode("")); // Reserved2
    out
}

/// `\1Ole10Native` — OLENativeStream (2.3.6): the native data size followed by
/// the native data. The native data of a spreadsheet object is its workbook
/// file, so pasting embeds the actual .xlsx bytes.
pub fn build_ole10_native(workbook: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + workbook.len());
    out.extend_from_slice(&(workbook.len() as u32).to_le_bytes());
    out.extend_from_slice(workbook);
    out
}

/// `\3OlePres000` — OLEPresentationStream (2.3.4) with an Enhanced metafile
/// presentation (the spec's generic clipboard-format form; CF_ENHMETAFILE is
/// explicitly supported there). Width/height are HIMETRIC (0.01 mm units,
/// which is exactly the EMF frame's units).
pub fn build_ole_pres(emf: &[u8], width_himetric: u32, height_himetric: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 4 + 4 + 4 + 4 + 4 + 4 + 4 + 4 + 8 + emf.len());
    out.extend_from_slice(&CF_MARKER_STANDARD.to_le_bytes());
    out.extend_from_slice(&CF_ENHMETAFILE.to_le_bytes()); // AnsiClipboardFormat
    out.extend_from_slice(&4u32.to_le_bytes()); // TargetDeviceSize: none
    out.extend_from_slice(&ASPECT_CONTENT.to_le_bytes());
    out.extend_from_slice(&u32::MAX.to_le_bytes()); // Lindex: -1
    out.extend_from_slice(&0u32.to_le_bytes()); // Advf
    out.extend_from_slice(&0u32.to_le_bytes()); // Reserved1
    out.extend_from_slice(&width_himetric.to_le_bytes());
    out.extend_from_slice(&height_himetric.to_le_bytes());
    out.extend_from_slice(&(emf.len() as u32).to_le_bytes()); // Size
    out.extend_from_slice(emf);
    out.extend_from_slice(&OLEPRES_TOC_SIGNATURE.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // TocCount: none
    out
}

/// `Object Descriptor` — OBJECTDESCRIPTOR (ole2.h): the fixed 52-byte
/// structure followed by ANSI string data at the offsets its last two fields
/// name. Sizel is the object's display size in HIMETRIC.
pub fn build_object_descriptor(
    width_himetric: u32,
    height_himetric: u32,
    user_type: &str,
    source_name: &str,
) -> Vec<u8> {
    const HEADER_LEN: usize = 52;
    let user_type_offset = if user_type.is_empty() {
        0
    } else {
        HEADER_LEN as u32
    };
    let source_offset = if source_name.is_empty() {
        0
    } else {
        HEADER_LEN as u32 + (user_type.len() + 1) as u32
    };
    let total = HEADER_LEN + user_type.len() + 1 + source_name.len() + 1;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&(total as u32).to_le_bytes()); // cbSize
    out.extend_from_slice(GENOFFICE_SHEET_CLSID.as_bytes()); // clsid
    out.extend_from_slice(&ASPECT_CONTENT.to_le_bytes()); // dwDrawAspect
    out.extend_from_slice(&width_himetric.to_le_bytes()); // sizel.cx
    out.extend_from_slice(&height_himetric.to_le_bytes()); // sizel.cy
    out.extend_from_slice(&0i32.to_le_bytes()); // pointl.x
    out.extend_from_slice(&0i32.to_le_bytes()); // pointl.y
    out.extend_from_slice(&0u32.to_le_bytes()); // dwStatus
    out.extend_from_slice(&user_type_offset.to_le_bytes());
    out.extend_from_slice(&source_offset.to_le_bytes());
    if !user_type.is_empty() {
        out.extend_from_slice(user_type.as_bytes());
        out.push(0);
    }
    if !source_name.is_empty() {
        out.extend_from_slice(source_name.as_bytes());
        out.push(0);
    }
    debug_assert_eq!(out.len(), total);
    out
}

/// Builds the `Embed Source` compound file: root storage carries the class
/// GUID (what OleLoad resolves for activation), and the four streams make the
/// object a first-class OLE embedding of the workbook.
pub fn build_embed_source(
    workbook: &[u8],
    emf: &[u8],
    width_himetric: u32,
    height_himetric: u32,
) -> Result<Vec<u8>, String> {
    let user_type = "GenOffice Worksheet";
    let cursor = Cursor::new(Vec::<u8>::new());
    // V3 (512-byte sectors) is what OLE objects in the wild use; consumers'
    // StgOpenStorage reads both, so stay with the common denominator.
    let mut compound = cfb::CompoundFile::create_with_version(cfb::Version::V3, cursor)
        .map_err(|e| format!("cfb create failed: {e}"))?;
    for (name, bytes) in [
        ("/\u{1}Ole", build_ole_stream()),
        ("/\u{1}CompObj", build_comp_obj(user_type)),
        ("/\u{1}Ole10Native", build_ole10_native(workbook)),
        (
            "/\u{3}OlePres000",
            build_ole_pres(emf, width_himetric, height_himetric),
        ),
    ] {
        let mut stream = compound
            .create_stream(name)
            .map_err(|e| format!("cfb stream {name} failed: {e}"))?;
        stream
            .write_all(&bytes)
            .map_err(|e| format!("cfb write {name} failed: {e}"))?;
    }
    compound
        .set_storage_clsid("/", GENOFFICE_SHEET_CLSID)
        .map_err(|e| format!("cfb clsid failed: {e}"))?;
    // into_inner flushes the directory and FAT before handing the cursor back.
    let mut cursor = compound.into_inner();
    cursor.set_position(0);
    let mut bytes = Vec::new();
    cursor
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cfb read-back failed: {e}"))?;
    Ok(bytes)
}

/// Decodes the payload's optional workbook attachment. Returns None when the
/// renderer/main did not send one (unsaved workbook, or over the size cap)
/// and the copy should carry only the EMF.
pub fn decode_workbook(workbook_base64: Option<&String>, cap: usize) -> Option<Vec<u8>> {
    let raw = workbook_base64?;
    if raw.is_empty() {
        return None;
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(raw)
        .ok()
        .filter(|bytes| !bytes.is_empty() && bytes.len() <= cap)?;
    Some(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER_TYPE: &str = "GenOffice Worksheet";

    #[test]
    fn ole_stream_is_the_twenty_byte_embedded_header() {
        let bytes = build_ole_stream();
        assert_eq!(bytes.len(), 20);
        assert_eq!(&bytes[0..4], &0x0200_0001u32.to_le_bytes());
        assert_eq!(&bytes[4..8], &0u32.to_le_bytes()); // embedded flag
        // every trailing field zero: no monikers on an embedded object
        assert!(bytes[8..].iter().all(|b| *b == 0));
    }

    #[test]
    fn comp_obj_carries_the_user_type_and_the_emf_format() {
        let bytes = build_comp_obj(USER_TYPE);
        let ansi_type = lp_ansi(USER_TYPE);
        // header (28) + ansi user type + cf marker pair + empty lpas; the
        // unicode marker sits right after that
        let offset = 28 + ansi_type.len() + 8 + 4;
        assert_eq!(
            &bytes[offset..offset + 4],
            &COMPOBJ_UNICODE_MARKER.to_le_bytes()
        );
        // the CF_ENHMETAFILE id appears in both halves
        let id = CF_ENHMETAFILE.to_le_bytes();
        assert_eq!(bytes.windows(4).filter(|w| *w == id).count(), 2);
        // and the unicode user type is present after the marker
        let unicode_type = lp_unicode(USER_TYPE);
        assert_eq!(
            &bytes[offset + 4..offset + 4 + unicode_type.len()],
            &unicode_type[..]
        );
    }

    #[test]
    fn ole10_native_prefixes_the_workbook_size() {
        let bytes = build_ole10_native(b"PK\x03\x04workbook");
        assert_eq!(&bytes[0..4], &12u32.to_le_bytes());
        assert_eq!(&bytes[4..], b"PK\x03\x04workbook");
    }

    #[test]
    fn ole_pres_declares_enhmetafile_and_carries_the_emf() {
        let emf = vec![7u8; 100];
        let bytes = build_ole_pres(&emf, 12_345, 6_789);
        assert_eq!(
            &bytes[0..8],
            &[CF_MARKER_STANDARD, CF_ENHMETAFILE]
                .map(u32::to_le_bytes)
                .concat()[..]
        );
        assert_eq!(&bytes[8..12], &4u32.to_le_bytes()); // TargetDeviceSize
        assert_eq!(&bytes[12..16], &ASPECT_CONTENT.to_le_bytes());
        assert_eq!(&bytes[16..20], &u32::MAX.to_le_bytes()); // Lindex
        assert_eq!(&bytes[28..32], &12_345u32.to_le_bytes()); // Width
        assert_eq!(&bytes[32..36], &6_789u32.to_le_bytes()); // Height
        assert_eq!(&bytes[36..40], &100u32.to_le_bytes()); // Size
        assert_eq!(&bytes[40..140], &emf[..]);
        assert_eq!(&bytes[140..144], &OLEPRES_TOC_SIGNATURE.to_le_bytes());
        assert_eq!(&bytes[144..148], &0u32.to_le_bytes()); // TocCount
    }

    #[test]
    fn object_descriptor_strings_sit_at_their_offsets() {
        let bytes = build_object_descriptor(100, 200, USER_TYPE, "plan.xlsx");
        let cb = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        assert_eq!(cb as usize, bytes.len());
        assert_eq!(bytes[4..20], *GENOFFICE_SHEET_CLSID.as_bytes());
        let user_offset = u32::from_le_bytes(bytes[44..48].try_into().unwrap()) as usize;
        let source_offset = u32::from_le_bytes(bytes[48..52].try_into().unwrap()) as usize;
        assert_eq!(
            &bytes[user_offset..user_offset + USER_TYPE.len()],
            USER_TYPE.as_bytes()
        );
        assert_eq!(&bytes[source_offset..source_offset + 9], b"plan.xlsx");
        // the descriptor strings are null-terminated
        assert_eq!(bytes[user_offset + USER_TYPE.len()], 0);
        assert_eq!(bytes[source_offset + 9], 0);
    }

    #[test]
    fn embed_source_is_a_readable_compound_file_with_all_streams() {
        let workbook = vec![0x50u8, 0x4B, 0x03, 0x04, 1, 2, 3, 4, 5];
        let emf = vec![9u8; 40];
        let bytes = build_embed_source(&workbook, &emf, 500, 300).unwrap();
        let mut cursor = Cursor::new(bytes);
        cursor.set_position(0);
        let mut compound = cfb::OpenOptions::new().open_with(cursor).unwrap();
        // root storage CLSID is what OleLoad resolves
        assert_eq!(compound.root_entry().clsid(), &GENOFFICE_SHEET_CLSID);
        let mut native = compound.open_stream("/\u{1}Ole10Native").unwrap();
        let mut native_bytes = Vec::new();
        std::io::Read::read_to_end(&mut native, &mut native_bytes).unwrap();
        assert_eq!(&native_bytes[4..], &workbook[..]);
        assert_eq!(&native_bytes[0..4], &(workbook.len() as u32).to_le_bytes());
        let mut pres_stream = compound.open_stream("/\u{3}OlePres000").unwrap();
        let mut pres = Vec::new();
        std::io::Read::read_to_end(&mut pres_stream, &mut pres).unwrap();
        // ten 4-byte header fields, the EMF, then the TOC pair
        assert_eq!(pres.len(), 40 + emf.len() + 8);
        assert!(compound.exists("/\u{1}CompObj"));
        assert!(compound.exists("/\u{1}Ole"));
    }

    #[test]
    fn workbook_decode_caps_and_rejects_garbage() {
        let good = base64::engine::general_purpose::STANDARD.encode(b"hello");
        assert_eq!(decode_workbook(Some(&good), 8), Some(b"hello".to_vec()));
        assert_eq!(decode_workbook(None, 8), None);
        assert_eq!(decode_workbook(Some(&String::new()), 8), None);
        assert_eq!(decode_workbook(Some(&"not base64!!".into()), 8), None);
        let big = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 9]);
        assert_eq!(decode_workbook(Some(&big), 8), None);
    }
}
