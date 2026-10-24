//! SemVer parsing for stable app releases. Build metadata does not affect ordering.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Version {
    core: (u64, u64, u64),
    pub pre: String,
}
impl Version {
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        let (version, build) = text
            .split_once('+')
            .map_or((text, None), |(v, b)| (v, Some(b)));
        fn identifiers(s: &str, prerelease: bool) -> bool {
            !s.is_empty()
                && s.split('.').all(|i| {
                    !i.is_empty()
                        && i.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                        && !(prerelease
                            && i.len() > 1
                            && i.starts_with('0')
                            && i.bytes().all(|b| b.is_ascii_digit()))
                })
        }
        if build.is_some_and(|b| !identifiers(b, false)) {
            return Err("Invalid SemVer build metadata");
        }
        let (core, pre) = version.split_once('-').unwrap_or((version, ""));
        if version.contains('-') && !identifiers(pre, true) {
            return Err("Invalid SemVer prerelease");
        }
        let parts: Vec<_> = core.split('.').collect();
        if parts.len() != 3 {
            return Err("Expected MAJOR.MINOR.PATCH");
        }
        let number = |s: &str| {
            if s.is_empty()
                || (s.len() > 1 && s.starts_with('0'))
                || !s.bytes().all(|b| b.is_ascii_digit())
            {
                return Err("Invalid SemVer number");
            }
            s.parse::<u64>().map_err(|_| "Invalid SemVer number")
        };
        Ok(Self {
            core: (number(parts[0])?, number(parts[1])?, number(parts[2])?),
            pre: pre.into(),
        })
    }
}
impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering::*;
        match self.core.cmp(&other.core) {
            Equal => {}
            order => return order,
        }
        match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => return Equal,
            (true, false) => return Greater,
            (false, true) => return Less,
            _ => {}
        }
        let mut a = self.pre.split('.');
        let mut b = other.pre.split('.');
        loop {
            match (a.next(), b.next()) {
                (None, None) => return Equal,
                (None, Some(_)) => return Less,
                (Some(_), None) => return Greater,
                (Some(a), Some(b)) => {
                    let an = a.bytes().all(|c| c.is_ascii_digit());
                    let bn = b.bytes().all(|c| c.is_ascii_digit());
                    let order = match (an, bn) {
                        (true, true) => a.len().cmp(&b.len()).then_with(|| a.cmp(b)),
                        (true, false) => Less,
                        (false, true) => Greater,
                        _ => a.cmp(b),
                    };
                    if order != Equal {
                        return order;
                    }
                }
            }
        }
    }
}
impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
