//! Ported from kaseer/src/core/escpos.test.ts. The oracle's "codepage
//! option accepted" case has no Rust counterpart: escpos_bytes takes no
//! options (cp858 is the only codepage and is selected unconditionally).

use kaseer_core::escpos::{escpos_bytes, escpos_to_data_url};

const CRLF: &str = "KASEER\r\nLine two\r\nNo trailing newline";

#[test]
fn starts_with_esc_at_init() {
    let bytes = escpos_bytes("hello");
    assert_eq!(&bytes[..2], &[0x1b, 0x40]);
}

#[test]
fn selects_codepage_858_after_init() {
    assert_eq!(&escpos_bytes("x")[2..5], &[0x1b, 0x74, 0x0d]);
}

#[test]
fn ends_with_full_cut_gs_v_66_0() {
    let bytes = escpos_bytes("hello");
    assert_eq!(&bytes[bytes.len() - 4..], &[0x1d, 0x56, 0x42, 0x00]);
}

#[test]
fn normalizes_crlf_and_cr_to_lf_only() {
    let bytes = escpos_bytes(CRLF);
    let body = &bytes[5..bytes.len() - 4];
    assert!(!body.contains(&0x0d));
    assert_eq!(String::from_utf8(body.to_vec()).unwrap(), "KASEER\nLine two\nNo trailing newline");
}

#[test]
fn data_url_roundtrips_through_base64() {
    use base64::Engine as _;
    let bytes = escpos_bytes("KASEER\nRM1.00\n");
    let url = escpos_to_data_url(&bytes);
    assert!(url.starts_with("data:application/octet-stream;base64,"));
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&url[url.find(',').unwrap() + 1..])
        .unwrap();
    assert_eq!(decoded, bytes);
}
