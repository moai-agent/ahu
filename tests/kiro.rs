mod common;

#[test]
fn native_shortcut_uses_installed_metadata_without_builtin_capabilities() {
    assert!(matches!(ahu::cli::parse(["kiro"]).unwrap(),
        ahu::cli::Command::Harness { selector, .. } if selector == "kiro"));
    assert!(ahu::cli::parse(["kiro", "--model", "auto"]).is_err());
    let repo = common::TestRepo::new();
    let discovered = ahu::git::discover(repo.path()).unwrap();
    assert!(ahu::harness_runtime::load(&discovered, "kiro").is_err());
    repo.install_runtime("kiro");
    repo.commit("installed synthetic runtime");
    let runtime = ahu::harness_runtime::load(&discovered, "kiro").unwrap();
    assert_eq!(runtime.descriptor().id, "kiro");
    assert_eq!(runtime.descriptor().program, "kiro-cli");
}
