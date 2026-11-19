pub fn read_cpu_sample() -> (u64, u64) {
    let mut idle: u64 = 0;
    let mut total: u64 = 0;
    if let Ok(content) = std::fs::read_to_string("/proc/stat")
        && let Some(line) = content.lines().next()
    {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let nums: Vec<u64> = parts[1..].iter().filter_map(|s| s.parse().ok()).collect();
        idle = nums.get(3).copied().unwrap_or(0) + nums.get(4).copied().unwrap_or(0);
        total = nums.iter().sum();
    }
    (idle, total)
}

pub fn read_mem_info() -> (f64, f64, f64) {
    let mut total: u64 = 0;
    let mut avail: u64 = 0;
    if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
        for line in content.lines() {
            if line.starts_with("MemTotal:") {
                total = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
            } else if line.starts_with("MemAvailable:") {
                avail = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
            }
        }
    }
    let total_gb = total as f64 / 1024.0 / 1024.0;
    let used_gb = (total.saturating_sub(avail)) as f64 / 1024.0 / 1024.0;
    let pct = if total > 0 {
        (total.saturating_sub(avail)) as f64 / total as f64 * 100.0
    } else {
        0.0
    };
    (used_gb, total_gb, pct)
}

pub fn read_battery() -> Option<(u8, String)> {
    for name in ["BAT0", "BAT1", "BAT2"] {
        let base = format!("/sys/class/power_supply/{}", name);
        let cap_path = format!("{}/capacity", base);
        let status_path = format!("{}/status", base);
        if let Ok(cap_str) = std::fs::read_to_string(&cap_path)
            && let Ok(cap) = cap_str.trim().parse::<u8>()
        {
            let status = std::fs::read_to_string(&status_path)
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            return Some((cap, status));
        }
    }
    None
}
