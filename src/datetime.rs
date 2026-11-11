use std::mem::MaybeUninit;
use std::time::{SystemTime, UNIX_EPOCH};

fn local_parts(time: SystemTime) -> Option<libc::tm> {
    let seconds: libc::time_t = time
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs()
        .try_into()
        .ok()?;
    let mut result = MaybeUninit::<libc::tm>::uninit();
    let local = unsafe { libc::localtime_r(&seconds, result.as_mut_ptr()) };
    (!local.is_null()).then(|| unsafe { result.assume_init() })
}

pub fn hhmm(time: SystemTime) -> Option<String> {
    let local = local_parts(time)?;
    Some(format!("{:02}:{:02}", local.tm_hour, local.tm_min))
}

pub fn suggestion(word: &str) -> Option<String> {
    if !matches!(word, "datetime" | "timeHMS") {
        return None;
    }
    let local = local_parts(SystemTime::now())?;
    match word {
        "datetime" => Some(format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            local.tm_year + 1900,
            local.tm_mon + 1,
            local.tm_mday,
            local.tm_hour,
            local.tm_min
        )),
        "timeHMS" => Some(format!(
            "{:02}:{:02}:{:02}",
            local.tm_hour, local.tm_min, local.tm_sec
        )),
        _ => None,
    }
}
