#[test]
fn pair_bound_write_rejects_independent_payload_and_target() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/write_permit_pair_mismatch.rs");
}

#[test]
fn pending_deletion_record_has_no_unconditional_checksum() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/pending_deletion_checksum.rs");
}
