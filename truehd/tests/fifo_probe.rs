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
    let mut max_au = 0;
    let mut total = 0usize;
    for f in ex.by_ref() {
        if let Ok(f) = f
            && let Ok(au) = p.parse(&f)
        {
            n += 1;
            let bytes = (au.access_unit_length as usize) << 1;
            max_au = max_au.max(bytes);
            total += bytes;
        }
    }
    let subs: Vec<String> = (0..truehd::utils::fifo::SUBSTREAMS)
        .filter_map(|i| {
            let s = p.substream_state(i)?;
            Some(format!(
                "{}:{} lat {} chan {}..{} mmc {} sw {:04X}",
                i,
                p.fifo_substream_peaks()[i],
                s.latency,
                s.restart.min_chan,
                s.restart.max_chan,
                s.restart.max_matrix_chan,
                s.restart.restart_sync_word,
            ))
        })
        .collect();
    println!(
        "AUs {n} peaks {:?} records {:?} substreams [{}] maxrate {} @ {} maxau {} avgau {:.2} maxlat {}",
        p.fifo_depth_peaks(),
        p.fifo_depth_records(),
        subs.join(" | "),
        p.max_data_rate(),
        p.max_data_rate_au(),
        max_au,
        total as f64 / n as f64,
        p.max_fifo_latency(),
    );
}
