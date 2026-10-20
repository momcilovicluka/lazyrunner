//! The platform lazyrunner is built for, so the self-update downloads that platform's release.

fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    println!("cargo:rustc-env=LAZYRUNNER_TARGET={target}");
}
