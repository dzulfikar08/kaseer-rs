//! Minimal pure ESC/POS byte encoder for thermal receipt printers.
//!
//! Emits: hardware init (ESC @), CP858 codepage select (ESC t 13), the
//! receipt text with LF-normalized line endings, and a full paper cut
//! (GS V 66 0). No raster commands, no status polling. Text is passed
//! through as UTF-8 bytes; ASCII renders identically under CP858.

const CUT: [u8; 4] = [0x1d, 0x56, 0x42, 0x00];

pub fn escpos_bytes(text: &str) -> Vec<u8> {
    // Only cp858 is defined today; the codepage select below is unconditional.
    let body = text.replace("\r\n", "\n").replace('\r', "\n").into_bytes();

    let mut out = Vec::with_capacity(2 + 3 + body.len() + CUT.len());
    out.extend_from_slice(&[0x1b, 0x40]); // ESC @ : initialize printer
    out.extend_from_slice(&[0x1b, 0x74, 0x0d]); // ESC t 13 : select codepage 858
    out.extend_from_slice(&body);
    out.extend_from_slice(&CUT); // GS V 66 0 : full cut
    out
}

pub fn escpos_to_data_url(bytes: &[u8]) -> String {
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    format!("data:application/octet-stream;base64,{}", encoded)
}
