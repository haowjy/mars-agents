// The .stderr snapshots pin rustc's diagnostic wording, which changes on nightly; the
// type-level guarantee they check is the same on every toolchain, so stable is enough.
#[rustversion::attr(
    not(stable),
    ignore = "compile-fail snapshots match stable rustc diagnostics"
)]
#[test]
fn pending_deletion_record_has_no_unconditional_checksum() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/pending_deletion_checksum.rs");
}
