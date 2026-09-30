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

#[cfg(test)]
mod tests {
    #[test]
    fn formats_are_zero_padded_and_plausible() {
        let t = super::now();
        assert!(t.year >= 2024 && (1..=12).contains(&t.month) && (1..=31).contains(&t.day));
        assert_eq!(t.hm().len(), 5);
        assert_eq!(t.date().len(), 10);
    }
}
