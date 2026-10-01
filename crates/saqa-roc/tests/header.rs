//! The SDK's `saqa/stream.h` writes the same wire by hand: its constants must
//! stay these. This reads the header, so it holds without libroc or a C++
//! compiler; saqa-stream's tests/sdk.rs checks the two against each other on
//! real links.

use saqa_roc::{multitrack_id, MAX_CHANNELS, PACKET_PAYLOAD, SAMPLE_RATE};

const HEADER: &str = include_str!("../../../sdk/cpp/include/saqa/stream.h");

fn has(line: &str) {
    assert!(
        HEADER.lines().any(|l| l.trim() == line),
        "sdk/cpp/include/saqa/stream.h no longer has `{line}`: change it together with saqa-roc"
    );
}

#[test]
fn the_sdk_header_speaks_the_same_wire() {
    has(&format!("constexpr unsigned kStreamRate = {SAMPLE_RATE};"));
    has(&format!(
        "constexpr unsigned kStreamMaxChannels = {MAX_CHANNELS};"
    ));
    assert_eq!(multitrack_id(0), 100);
    has(
        "inline int multitrackId (unsigned channels) { return 100 + static_cast<int> (channels); }",
    );
    has(&format!(
        "unsigned frames = {PACKET_PAYLOAD}u / (channels * 4u);"
    ));
    has(r#"f (ROC_INTERFACE_AUDIO_SOURCE, "rtp+rs8m://" + at (port));"#);
    has(r#"f (ROC_INTERFACE_AUDIO_REPAIR, "rs8m://" + at (port + 1));"#);
    has(r#"f (ROC_INTERFACE_AUDIO_CONTROL, "rtcp://" + at (port + 2));"#);
}
