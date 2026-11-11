use super::*;
use serde_json::json;

fn packet() -> Value {
    json!({"schemaVersion":1,"configuration":{"private":"memory-only"},"secrets":{"token":"private-token"}})
}

#[test]
fn initialized_packet_is_bounded_typed_and_never_echoes_invalid_values() {
    assert!(Initialized::parse(&serde_json::to_vec(&packet()).unwrap()).is_ok());
    for (field, value) in [
        ("schemaVersion", json!(2)),
        ("schemaVersion", json!("private-token")),
        ("configuration", json!("private-token")),
        (
            "configuration",
            json!({"value":"x".repeat(contracts::MAX_CONFIGURATION)}),
        ),
        ("secrets", json!({"token":true})),
        (
            "secrets",
            json!({"token":"x".repeat(contracts::MAX_SECRETS)}),
        ),
        ("unknown-private-field", json!("private-token")),
    ] {
        let mut input = packet();
        input[field] = value;
        assert_eq!(
            Initialized::parse(&serde_json::to_vec(&input).unwrap())
                .err()
                .unwrap()
                .to_string(),
            "Invalid initialized runner input"
        );
    }
    let duplicate = br#"{"schemaVersion":1,"schemaVersion":1,"configuration":{},"secrets":{}}"#;
    assert!(Initialized::parse(duplicate).is_err());
}

#[tokio::test]
async fn initialized_read_requires_a_complete_single_document_before_admission() {
    let mode = Mode::Initialize;
    for bytes in [b"{}".as_slice(), b"{\"configuration\":", b"\xff", b"{}\n{}"] {
        assert!(mode.read(&mut BufReader::new(bytes)).await.is_err());
    }
    let bytes = serde_json::to_vec(&packet()).unwrap();
    assert!(mode
        .read(&mut BufReader::new(bytes.as_slice()))
        .await
        .is_ok());
    let oversized = vec![b' '; contracts::MAX_INPUT + 1];
    assert!(mode
        .read(&mut BufReader::new(oversized.as_slice()))
        .await
        .unwrap_err()
        .to_string()
        .contains("exceeds limit"));
}

#[test]
fn initialized_arguments_cannot_select_file_fallback_or_ambiguous_options() {
    for values in [
        vec![
            "start",
            "--bootstrap",
            "initialize-v1",
            "--config",
            "private",
        ],
        vec![
            "start",
            "--bootstrap",
            "initialize-v1",
            "--bootstrap",
            "initialize-v1",
        ],
        vec![
            "start",
            "--bootstrap",
            "initialize-v1",
            "--unknown",
            "private",
        ],
        vec!["start", "--bootstrap", "future"],
        vec!["start", "--bootstrap"],
    ] {
        let values = values.into_iter().map(String::from).collect::<Vec<_>>();
        assert!(Mode::select(&values).is_err());
    }
    assert!(matches!(
        Mode::select(&["start".into(), "--bootstrap".into(), "initialize-v1".into()]).unwrap(),
        Mode::Initialize
    ));
    assert!(matches!(
        Mode::select(&["start".into(), "--config".into(), "original-file".into()]).unwrap(),
        Mode::File(_)
    ));
}
