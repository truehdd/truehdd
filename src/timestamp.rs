pub fn time_str(sec: f64) -> String {
    let ms = sec * 1000f64;
    let hours = (ms / 3600000f64) as u64;
    let minutes = ((ms % 3600000f64) / 60000f64) as u64;
    let seconds = ((ms % 60000f64) / 1000f64) as u64;
    let milliseconds = (ms % 1000f64) as u64;

    format!(
        "{hours:0width$}:{minutes:02}:{seconds:02}.{milliseconds:03}",
        width = if hours >= 100 { 0 } else { 2 }
    )
}
