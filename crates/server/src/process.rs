//! Facts about the server process, as reported by the operating system.

/// Resident memory, from `/proc` on Linux; `None` on other platforms.
pub fn resident_memory_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    vm_rss_bytes(&status)
}

fn vm_rss_bytes(status: &str) -> Option<u64> {
    let kib = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .trim()
        .strip_suffix("kB")?
        .trim()
        .parse::<u64>()
        .ok()?;
    Some(kib * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_resident_memory_from_proc_status() {
        let status = "Name:\tserver\nVmPeak:\t  200000 kB\nVmRSS:\t   65432 kB\nThreads:\t9\n";

        assert_eq!(vm_rss_bytes(status), Some(65_432 * 1024));
        assert_eq!(vm_rss_bytes("Name:\tserver\n"), None);
    }
}
