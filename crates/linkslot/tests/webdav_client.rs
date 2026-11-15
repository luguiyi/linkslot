//! Direct tests for the WebDAV client against the in-process mock server:
//! 404 handling, Chinese percent-encoded paths, Basic auth, retries and the
//! MOVE/MKCOL/DELETE primitives.

mod support;

use linkslot::error::ErrorKind;
use linkslot::output::Reporter;
use linkslot::webdav::{base64_encode, DavClient, MoveOutcome};
use support::*;

const UNIT_SEGMENTS: [&str; 3] = ["GBA", "mgba", GAME];

fn with_reporter<F>(harness: &Harness, body: F)
where
    F: FnOnce(&mut Reporter),
{
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let mut reporter = Reporter::new(&harness.root, true, &mut out, &mut err);
    body(&mut reporter);
}

#[test]
fn get_put_roundtrip_with_chinese_segments() {
    let harness = Harness::new("dav-roundtrip");
    with_reporter(&harness, |rep| {
        let client = DavClient::new(&harness.config, rep).unwrap();
        client.ensure_dir(rep, &UNIT_SEGMENTS).unwrap();
        let rel = ["GBA", "mgba", GAME, "resume.state"];
        client.put(rep, &rel, b"hello-state").unwrap();
        let bytes = client.get(rep, &rel).unwrap().unwrap();
        assert_eq!(bytes, b"hello-state");

        let missing = client
            .get(rep, &["GBA", "mgba", GAME, "nope.state"])
            .unwrap();
        assert!(missing.is_none());

        // Remote root collection and unit directory were created.
        let state = harness.dav.state();
        let state = state.lock().unwrap();
        assert!(state.dirs.contains("/linkSlot"));
        assert!(state.dirs.contains("/linkSlot/GBA/mgba/合金弹头"));
        let put = state
            .requests
            .iter()
            .find(|record| record.method == "PUT")
            .unwrap();
        assert!(
            put.raw_path
                .contains("%E5%90%88%E9%87%91%E5%BC%B9%E5%A4%B4"),
            "raw path not percent encoded: {}",
            put.raw_path
        );
        assert_eq!(put.path, "/linkSlot/GBA/mgba/合金弹头/resume.state");
        let expected = format!("Basic {}", base64_encode(b"user:pass"));
        assert_eq!(put.auth.as_deref(), Some(expected.as_str()));
    });
}

#[test]
fn get_missing_meta_returns_none_without_error() {
    let harness = Harness::new("dav-404");
    with_reporter(&harness, |rep| {
        let client = DavClient::new(&harness.config, rep).unwrap();
        let meta = client
            .get(rep, &["GBA", "mgba", GAME, "meta.json"])
            .unwrap();
        assert!(meta.is_none());
    });
}

#[test]
fn auth_failure_is_classified_as_auth() {
    let harness = Harness::new("dav-auth");
    harness.dav.set_auth("user", "changed-password");
    with_reporter(&harness, |rep| {
        let client = DavClient::new(&harness.config, rep).unwrap();
        let err = client
            .get(rep, &["GBA", "mgba", GAME, "resume.state"])
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Auth);
    });
}

#[test]
fn retries_transient_server_errors() {
    let harness = Harness::with_options("dav-retry", 1, false);
    harness
        .dav
        .insert_file("/linkSlot/GBA/mgba/合金弹头/resume.state", b"payload");
    harness.dav.fail_next("GET", 500, 1);
    with_reporter(&harness, |rep| {
        let client = DavClient::new(&harness.config, rep).unwrap();
        let bytes = client
            .get(rep, &["GBA", "mgba", GAME, "resume.state"])
            .unwrap()
            .unwrap();
        assert_eq!(bytes, b"payload");
    });
    let gets = harness
        .dav
        .requests()
        .iter()
        .filter(|record| record.method == "GET")
        .count();
    assert_eq!(gets, 2, "expected one failed + one successful GET");
}

#[test]
fn move_reports_unsupported_and_moves_when_supported() {
    let harness = Harness::new("dav-move");
    with_reporter(&harness, |rep| {
        let client = DavClient::new(&harness.config, rep).unwrap();
        client.ensure_dir(rep, &UNIT_SEGMENTS).unwrap();
        let tmp = ["GBA", "mgba", GAME, "resume.state.tmp"];
        let final_rel = ["GBA", "mgba", GAME, "resume.state"];
        client.put(rep, &tmp, b"moved-bytes").unwrap();
        assert_eq!(
            client.move_to(rep, &tmp, &final_rel).unwrap(),
            MoveOutcome::Moved
        );
        assert_eq!(
            client.get(rep, &final_rel).unwrap().unwrap(),
            b"moved-bytes"
        );

        harness.dav.set_move_supported(false);
        client.put(rep, &tmp, b"fallback").unwrap();
        assert_eq!(
            client.move_to(rep, &tmp, &final_rel).unwrap(),
            MoveOutcome::Unsupported
        );
    });
}

#[test]
fn mkcol_is_idempotent_and_delete_reports_presence() {
    let harness = Harness::new("dav-mkcol");
    with_reporter(&harness, |rep| {
        let client = DavClient::new(&harness.config, rep).unwrap();
        client.ensure_dir(rep, &UNIT_SEGMENTS).unwrap();
        // Second ensure: every MKCOL answers 405 and is accepted.
        client.ensure_dir(rep, &UNIT_SEGMENTS).unwrap();

        let rel = ["GBA", "mgba", GAME, "resume.state"];
        client.put(rep, &rel, b"x").unwrap();
        assert!(client.delete(rep, &rel).unwrap());
        assert!(!client.delete(rep, &rel).unwrap());
    });
}

#[test]
fn endpoint_and_url_building_are_per_segment_encoded() {
    let harness = Harness::new("dav-url");
    with_reporter(&harness, |rep| {
        let client = DavClient::new(&harness.config, rep).unwrap();
        let url = client.url_for(&["GBA", "mgba", GAME, "resume.state"]);
        assert_eq!(
            url,
            format!(
                "{}/linkSlot/GBA/mgba/%E5%90%88%E9%87%91%E5%BC%B9%E5%A4%B4/resume.state",
                harness.dav.url()
            )
        );
    });
}
