// kun: `build.rs`'s unit tests, as their own test target. Upstream's
// `[[test]]` pointed at `build.rs` itself, which Cargo (1.99) warns about:
// the file was then in two targets, the build script and this test.
// `main` is the build script's, unused here.
#[allow(dead_code)]
#[path = "../build.rs"]
mod build;
