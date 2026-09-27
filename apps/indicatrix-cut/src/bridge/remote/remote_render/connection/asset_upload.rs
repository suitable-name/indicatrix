//! Answering a server's `NEED_ASSET` (protocol v14): the scene names an HDR map
//! by content hash and the server does not have it yet, so the connection thread that
//! owns the stream sends the exact bytes (`indicatrix_net::client::send_asset`) before
//! reading on. Both connection lifecycles (one-shot and persistent) answer through
//! [`send_requested_asset`].

use crate::bridge::remote::hdr_asset;
use indicatrix_net::messages::{ContentHash, hash_hex};
use std::io::Write;

/// Sends the bytes of the loaded HDR map whose SHA-256 is `hash` as one `ASSET`
/// message.
///
/// # Errors
///
/// A human-readable reason when no loaded map has that hash, its file cannot be re-read
/// or changed since it was loaded (see `hdr_asset::HdrAsset::upload_bytes`), or writing
/// fails. The caller fails the request with it.
pub(super) fn send_requested_asset<W: Write>(
    stream: &mut W,
    hash: &ContentHash,
) -> Result<(), String> {
    let asset = hdr_asset::asset_by_hash(hash).ok_or_else(|| {
        format!(
            "the remote asked for HDR map {}, which is not loaded in this viewer",
            hash_hex(hash)
        )
    })?;
    let bytes = asset.upload_bytes()?;
    tracing::info!(
        "sending HDR map {} ({} bytes) to the remote",
        hash_hex(hash),
        bytes.len()
    );
    indicatrix_net::client::send_asset(stream, &bytes)
        .map_err(|e| format!("uploading the HDR map failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_net::messages::{ClientMessage, content_hash, read_asset_payload};

    #[test]
    fn a_loaded_map_is_sent_as_one_verified_asset_message() {
        let pixels = vec![image::Rgb([0.25f32, 0.5, 1.0]); 24];
        let mut bytes = Vec::new();
        image::codecs::hdr::HdrEncoder::new(&mut bytes)
            .encode(&pixels, 6, 4)
            .unwrap();
        let path = std::env::temp_dir().join(format!(
            "indicatrix-cut-asset-upload-{}.hdr",
            std::process::id()
        ));
        std::fs::write(&path, &bytes).unwrap();
        let (_map, asset) = hdr_asset::load_hdr_file(&path).unwrap();

        let mut wire = Vec::new();
        send_requested_asset(&mut wire, asset.content_hash()).unwrap();
        let mut cursor = std::io::Cursor::new(wire);
        let ClientMessage::Asset(header) =
            indicatrix_net::messages::read_message(&mut cursor).unwrap()
        else {
            panic!("expected an ASSET message");
        };
        assert_eq!(header.content_hash, content_hash(&bytes));
        assert_eq!(read_asset_payload(&mut cursor, &header).unwrap(), bytes);

        let err = send_requested_asset(&mut Vec::new(), &[0xCD; 32]).unwrap_err();
        assert!(err.contains("not loaded"), "{err}");
        let _ = std::fs::remove_file(&path);
    }
}
