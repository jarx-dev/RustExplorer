// rust-i18n compiles locales/*.yml in without telling cargo, so a changed text alone kept the old
// build (new keys showed as "main.pack.level"); a changed file in the folder now rebuilds the crate
fn main() {
    println!("cargo:rerun-if-changed=locales");
}
