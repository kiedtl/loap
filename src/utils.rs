pub fn fmt_email(email: &str) -> String {
    let mut s = String::new();
    for (i, ch) in email.chars().enumerate() {
        s.push(match ch {
            'a' => 'а',
            'd' => 'ԁ',
            'e' => 'е',
            'p' => 'р',
            'o' => ['о', 'ο', 'օ'][i % 3],
            'u' => ['ս', 'υ'][i % 2],
            '@' => '＠',
            '.' => '·',
            c => c,
        });

        if i % 3 == 0 {
            s.push('\u{200C}');
        }
    }
    s
}

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
    parts.truncate(3); // Prevent "3w 5d 12h 8m 34s", "3w 5d 12h" is good enough!
    parts.join(" ")
}
