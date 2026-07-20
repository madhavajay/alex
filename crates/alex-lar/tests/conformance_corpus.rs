//! Default-test entry point for the published conformance manifest and vectors.

#[allow(dead_code)]
#[path = "../examples/verify_conformance.rs"]
mod verify_conformance;

#[test]
fn published_corpus_verifies() {
    verify_conformance::run().unwrap();
}
