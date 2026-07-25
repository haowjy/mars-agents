#[test]
fn pair_bound_write_rejects_independent_payload_and_target() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/write_permit_pair_mismatch.rs");
}
