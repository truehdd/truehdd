//! Manual probe: prints the FIFO depth peaks of a stream, for comparison against an
//! encoder's own maximum-FIFO-depth report.
//!
//! `STREAM=<path> cargo test -p truehd --release --test fifo_probe -- --nocapture`
//!
//! `STREAM` may name several files separated by `,`; they are concatenated into one
//! access-unit sequence before parsing, which is how a seamless branch is exercised. The
//! probe also reports `invalid_branches`, the count of branch points that failed the
//! buffer-model checks.

#[test]
fn zz_fifo_peaks() {
    let Ok(paths) = std::env::var("STREAM") else {
        eprintln!("STREAM not set, skipping the FIFO probe");
        return;
    };

    let mut data = Vec::new();
    for path in paths.split(',') {
        data.extend_from_slice(&std::fs::read(path).unwrap());
    }

    let mut ex = truehd::process::extract::Extractor::default();
    ex.push_bytes(&data);
    let mut p = truehd::process::parse::Parser::default();
    let mut n = 0;
    for f in ex.by_ref() {
        if let Ok(f) = f
            && p.parse(&f).is_ok()
        {
            n += 1
        }
    }
    println!(
        "AUs {n} peaks {:?} invalid_branches {}",
        p.fifo_depth_peaks(),
        p.invalid_branches()
    );
}
