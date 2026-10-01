use std::fs;
use std::time::Instant;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ResourceSnapshot {
    pub cpu_percent: Option<u8>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
    pub upload_bytes_per_sec: Option<u64>,
    pub download_bytes_per_sec: Option<u64>,
}

#[derive(Debug, Default)]
pub struct ResourceSampler {
    previous_cpu: Option<(u64, u64)>,
    previous_network: Option<(u64, u64, Instant)>,
}

impl ResourceSampler {
    pub fn sample(&mut self) -> ResourceSnapshot {
        let mut snapshot = ResourceSnapshot::default();
        #[cfg(target_os = "linux")]
        {
            if let Ok(data) = fs::read_to_string("/proc/stat") {
                if let Some((total, idle)) = parse_cpu_line(data.lines().next().unwrap_or_default())
                {
                    snapshot.cpu_percent = self.previous_cpu.and_then(|(old_total, old_idle)| {
                        let total_delta = total.saturating_sub(old_total);
                        let idle_delta = idle.saturating_sub(old_idle);
                        (total_delta > 0).then(|| {
                            ((total_delta.saturating_sub(idle_delta) as f64 / total_delta as f64)
                                * 100.0)
                                .round()
                                .clamp(0.0, 100.0) as u8
                        })
                    });
                    self.previous_cpu = Some((total, idle));
                }
            }
            if let Ok(data) = fs::read_to_string("/proc/meminfo") {
                if let Some((total, available)) = parse_meminfo(&data) {
                    snapshot.memory_total_bytes = Some(total);
                    snapshot.memory_used_bytes = Some(total.saturating_sub(available));
                }
            }
            if let Ok(data) = fs::read_to_string("/proc/net/dev") {
                if let Some((receive, transmit)) = parse_network(&data) {
                    let now = Instant::now();
                    if let Some((old_receive, old_transmit, old_time)) = self.previous_network {
                        let seconds = now.duration_since(old_time).as_secs_f64();
                        if seconds > 0.0 {
                            snapshot.download_bytes_per_sec = Some(
                                (receive.saturating_sub(old_receive) as f64 / seconds).round()
                                    as u64,
                            );
                            snapshot.upload_bytes_per_sec = Some(
                                (transmit.saturating_sub(old_transmit) as f64 / seconds).round()
                                    as u64,
                            );
                        }
                    }
                    self.previous_network = Some((receive, transmit, now));
                }
            }
        }
        snapshot
    }
}

pub fn parse_cpu_line(line: &str) -> Option<(u64, u64)> {
    let mut fields = line.split_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    let values: Vec<u64> = fields.map(str::parse).collect::<Result<_, _>>().ok()?;
    if values.len() < 4 {
        return None;
    }
    let total = values.iter().take(8).copied().sum();
    let idle = values[3].saturating_add(values.get(4).copied().unwrap_or(0));
    Some((total, idle))
}

pub fn parse_meminfo(text: &str) -> Option<(u64, u64)> {
    let value = |name: &str| -> Option<u64> {
        text.lines().find_map(|line| {
            let (key, rest) = line.split_once(':')?;
            if key != name {
                return None;
            }
            rest.split_whitespace().next()?.parse::<u64>().ok()
        })
    };
    let total = value("MemTotal")?.checked_mul(1024)?;
    let available = value("MemAvailable")
        .or_else(|| value("MemFree"))?
        .checked_mul(1024)?;
    Some((total, available.min(total)))
}

pub fn parse_network(text: &str) -> Option<(u64, u64)> {
    let mut rows = text.lines().skip(2);
    let mut receive = 0u64;
    let mut transmit = 0u64;
    let mut found = false;
    for row in &mut rows {
        let (name, counters) = row.split_once(':')?;
        if name.trim() == "lo" {
            continue;
        }
        let values: Vec<u64> = counters
            .split_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .ok()?;
        if values.len() < 9 {
            continue;
        }
        receive = receive.saturating_add(values[0]);
        transmit = transmit.saturating_add(values[8]);
        found = true;
    }
    found.then_some((receive, transmit))
}

pub fn format_line(snapshot: ResourceSnapshot, width: usize) -> String {
    let cpu = snapshot
        .cpu_percent
        .map(|value| value.to_string())
        .unwrap_or_else(|| "--".to_owned());
    let (used, total) = match (snapshot.memory_used_bytes, snapshot.memory_total_bytes) {
        (Some(used), Some(total)) => (gib(used), gib(total)),
        _ => ("--".to_owned(), "--".to_owned()),
    };
    let upload = format_rate(snapshot.upload_bytes_per_sec);
    let download = format_rate(snapshot.download_bytes_per_sec);

    let verbose = format!("CPU {cpu}% · 内存 {used}/{total}G · 网络 ↑{upload} ↓{download} · 1s");
    if width >= 74 && display_width(&verbose) <= width {
        return verbose;
    }

    let full = format!("C{cpu}% M{used}/{total}G ↑{upload} ↓{download}");
    if display_width(&full) <= width {
        return full;
    }

    let compact = format!(
        "C{cpu}% M{}G ↑{} ↓{}",
        gib_integer(snapshot.memory_used_bytes),
        compact_rate(snapshot.upload_bytes_per_sec),
        compact_rate(snapshot.download_bytes_per_sec)
    );
    if display_width(&compact) <= width {
        return compact;
    }

    let memory = format!("C{cpu}% M{}G", gib_integer(snapshot.memory_used_bytes));
    if display_width(&memory) <= width {
        memory
    } else {
        format!("C{cpu}%")
    }
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| {
            let code = ch as u32;
            usize::from(matches!(
                code,
                0x1100..=0x115f
                    | 0x2329..=0x232a
                    | 0x2e80..=0xa4cf
                    | 0xac00..=0xd7a3
                    | 0xf900..=0xfaff
                    | 0xfe10..=0xfe19
                    | 0xfe30..=0xfe6f
                    | 0xff00..=0xff60
                    | 0xffe0..=0xffe6
                    | 0x20000..=0x3fffd
            )) + 1
        })
        .sum()
}

fn gib(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

fn gib_integer(bytes: Option<u64>) -> String {
    bytes
        .map(|bytes| ((bytes as f64 / (1024.0 * 1024.0 * 1024.0)).round() as u64).to_string())
        .unwrap_or_else(|| "--".to_owned())
}

fn format_rate(rate: Option<u64>) -> String {
    match rate {
        None => "--".to_owned(),
        Some(bytes) if bytes < 1024 => format!("{bytes}B/s"),
        Some(bytes) if bytes < 1024 * 1024 => format!("{:.0}K/s", bytes as f64 / 1024.0),
        Some(bytes) if bytes < 1024 * 1024 * 1024 => {
            format!("{:.0}M/s", bytes as f64 / (1024.0 * 1024.0))
        }
        Some(bytes) => format!("{:.1}G/s", bytes as f64 / (1024.0 * 1024.0 * 1024.0)),
    }
}

fn compact_rate(rate: Option<u64>) -> String {
    match rate {
        None => "--".to_owned(),
        Some(bytes) if bytes < 1024 => format!("{bytes}B/s"),
        Some(bytes) if bytes < 1024 * 1024 => format!("{:.0}K/s", bytes as f64 / 1024.0),
        Some(bytes) if bytes < 1024 * 1024 * 1024 => {
            format!("{:.0}M/s", bytes as f64 / (1024.0 * 1024.0))
        }
        Some(bytes) => format!("{:.0}G/s", bytes as f64 / (1024.0 * 1024.0 * 1024.0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_cpu_counters_and_excludes_idle_from_busy_time() {
        assert_eq!(parse_cpu_line("cpu  10 2 3 80 5 0 0 0"), Some((100, 85)));
        assert_eq!(parse_cpu_line("cpu 10 0 0 80 0 0 0 0 10 0"), Some((90, 80)));
        assert_eq!(parse_cpu_line("cpu0 10 2 3 80"), None);
        assert_eq!(parse_cpu_line("cpu 10 2"), None);
    }

    #[test]
    fn parses_available_memory_and_network_counters() {
        let mem = "MemTotal: 8192 kB\nMemFree: 1024 kB\nMemAvailable: 2048 kB\n";
        assert_eq!(parse_meminfo(mem), Some((8 * 1024 * 1024, 2 * 1024 * 1024)));
        let network = "Inter-| Receive | Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\nlo: 999 0 0 0 0 0 0 0 888 0 0 0 0 0 0 0\neth0: 1000 0 0 0 0 0 0 0 3000 0 0 0 0 0 0 0\n";
        assert_eq!(parse_network(network), Some((1000, 3000)));
    }

    #[test]
    fn compact_status_adapts_to_narrow_terminals_without_line_breaks() {
        let value = ResourceSnapshot {
            cpu_percent: Some(12),
            memory_used_bytes: Some(2_300_000_000),
            memory_total_bytes: Some(8_000_000_000),
            upload_bytes_per_sec: Some(24 * 1024),
            download_bytes_per_sec: Some(180 * 1024),
        };
        let narrow = format_line(value, 32);
        let wide = format_line(value, 100);
        assert!(!narrow.contains('\n'));
        assert!(!wide.contains('\n'));
        assert!(narrow.len() < wide.len());
        assert!(wide.contains("12%"));
        assert!(wide.contains("↑"));
        assert!(wide.contains("↓"));

        let large = ResourceSnapshot {
            cpu_percent: Some(100),
            memory_used_bytes: Some(1000 * 1024 * 1024 * 1024),
            memory_total_bytes: Some(2000 * 1024 * 1024 * 1024),
            upload_bytes_per_sec: Some(100 * 1024 * 1024),
            download_bytes_per_sec: Some(200 * 1024 * 1024),
        };
        for width in [16, 24, 32, 42, 74] {
            assert!(display_width(&format_line(large, width)) <= width);
        }
    }
}
