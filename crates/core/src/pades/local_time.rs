//! La hora local, para la fecha de la firma (`/M`) y el texto que se ve.

/// Un momento en hora local.
pub struct LocalTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// Diferencia con UTC, en minutos.
    pub offset: i32,
}

impl LocalTime {
    /// Como fecha PDF: D:AAAAMMDDHHmmSS+HH'mm'.
    pub fn pdf(&self) -> String {
        let sign = if self.offset < 0 { '-' } else { '+' };
        let o = self.offset.abs();
        format!("D:{:04}{:02}{:02}{:02}{:02}{:02}{sign}{:02}'{:02}'", self.year, self.month, self.day, self.hour, self.minute, self.second, o / 60, o % 60)
    }

    /// Para el texto de la firma: 2026-10-14 21:30:05 +02:00.
    pub fn shown(&self) -> String {
        let sign = if self.offset < 0 { '-' } else { '+' };
        let o = self.offset.abs();
        format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} {sign}{:02}:{:02}", self.year, self.month, self.day, self.hour, self.minute, self.second, o / 60, o % 60)
    }
}

pub fn now() -> LocalTime {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let offset = utc_offset(t);
    let (year, month, day, hour, minute, second) = civil(t + offset as i64 * 60);
    LocalTime { year, month, day, hour, minute, second, offset }
}

/// Fecha y hora de un instante (segundos desde 1970, sin zona).
fn civil(t: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    // Algoritmo de Howard Hinnant (días a fecha civil).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y } as i32;
    (y, m, d, (secs / 3600) as u32, (secs % 3600 / 60) as u32, (secs % 60) as u32)
}

#[cfg(unix)]
fn utc_offset(t: i64) -> i32 {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let tt = t as libc::time_t;
    if unsafe { libc::localtime_r(&tt, &mut tm) }.is_null() {
        return 0;
    }
    (tm.tm_gmtoff / 60) as i32
}

#[cfg(windows)]
fn utc_offset(_t: i64) -> i32 {
    #[repr(C)]
    struct SystemTime([u16; 8]);
    #[repr(C)]
    struct TimeZoneInformation {
        bias: i32,
        standard_name: [u16; 32],
        standard_date: SystemTime,
        standard_bias: i32,
        daylight_name: [u16; 32],
        daylight_date: SystemTime,
        daylight_bias: i32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetTimeZoneInformation(info: *mut TimeZoneInformation) -> u32;
    }
    let mut tz: TimeZoneInformation = unsafe { std::mem::zeroed() };
    // 2: horario de verano en vigor; 1: estándar; 0: sin cambios de hora.
    let r = unsafe { GetTimeZoneInformation(&mut tz) };
    let bias = tz.bias + if r == 2 { tz.daylight_bias } else if r == 1 { tz.standard_bias } else { 0 };
    -bias
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fechas() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil(1_791_998_400), (2026, 10, 14, 17, 20, 0));
        let t = LocalTime { year: 2026, month: 10, day: 14, hour: 21, minute: 5, second: 3, offset: 120 };
        assert_eq!(t.pdf(), "D:20261014210503+02'00'");
        assert_eq!(t.shown(), "2026-10-14 21:05:03 +02:00");
    }
}
