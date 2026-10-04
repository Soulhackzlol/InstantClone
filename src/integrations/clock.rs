//! Local wall-clock time for the `{time}` and `{date}` variables.
//!
//! std has no local time zone, so this asks the OS directly: GetLocalTime on
//! Windows, localtime_r elsewhere.

pub struct LocalTime {
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

impl LocalTime {
    /// `21:04`
    pub fn hm(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }

    /// `2026-09-30`
    pub fn date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

#[cfg(windows)]
pub fn now() -> LocalTime {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    // SAFETY: GetLocalTime only writes the SYSTEMTIME we hand it.
    let t = unsafe {
        let mut t = std::mem::zeroed();
        GetLocalTime(&mut t);
        t
    };
    LocalTime {
        year: t.wYear as u32,
        month: t.wMonth as u32,
        day: t.wDay as u32,
        hour: t.wHour as u32,
        minute: t.wMinute as u32,
    }
}

#[cfg(unix)]
pub fn now() -> LocalTime {
    // SAFETY: time() and localtime_r() write only into the locals we pass.
    let tm = unsafe {
        let secs = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&secs, &mut tm);
        tm
    };
    LocalTime {
        year: (tm.tm_year + 1900) as u32,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
    }
}

/// `2026-10-02T18:36:46Z` for a Unix time in seconds: what a Discord card's
/// timestamp wants (Discord shows it in each reader's own time zone).
pub fn iso_utc(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let secs = unix_secs % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn utc_timestamps_are_iso_8601() {
        assert_eq!(super::iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(super::iso_utc(1_790_966_206), "2026-10-02T18:36:46Z");
        assert_eq!(super::iso_utc(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn formats_are_zero_padded_and_plausible() {
        let t = super::now();
        assert!(t.year >= 2024 && (1..=12).contains(&t.month) && (1..=31).contains(&t.day));
        assert_eq!(t.hm().len(), 5);
        assert_eq!(t.date().len(), 10);
    }
}
