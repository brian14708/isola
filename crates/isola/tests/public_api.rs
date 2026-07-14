use isola::sandbox::SandboxOptions;

#[test]
fn sandbox_options_can_be_merged_with_borrowed_overrides() {
    let base = SandboxOptions::default().env("BASE", "base");
    let overrides = SandboxOptions::default().env("OVERRIDE", "override");

    let merged = base.merged_with(&overrides);

    let _ = (merged, overrides);
}
