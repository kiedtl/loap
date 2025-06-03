pub fn fmt_size(size: u32) -> String {
    let (prec, fac, suffix) = match size {
        0..1000 => (0, 1.0, ""),
        1000..1_000_000 => (1, 1000.0, " KB"),
        1_000_000..1_000_000_000 => (2, 1_000_000.0, " MB"),
        1_000_000_000..=u32::MAX => (3, 1_000_000_000.0, " GB"),
    };
    format!("{:.*}{suffix}", prec, (size as f32) / fac)
}

pub fn fmt_duration(seconds: i64) -> String {
    let times = [(1, 60, "s"), (60, 60, "m"), (60 * 60, 24, "h"),
        (24 * 60 * 60, 7, "d"), (7 * 24 * 60 * 60, 1, "w")];

    let mut parts = vec![];
    for (d, m, s) in &times {
        if seconds / d % m != 0 {
            parts.push(format!("{}{s}", seconds / d % m));
        }
    }

    parts.reverse();
    parts.join(" ")
}
