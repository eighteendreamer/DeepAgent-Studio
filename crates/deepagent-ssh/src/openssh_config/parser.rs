use super::{invalid, SshResult, MAX_CONFIG_BYTES};

pub(super) struct Entry {
    line: usize,
    key: String,
    values: Vec<String>,
}

pub(super) struct ParsedConfig {
    entries: Vec<Entry>,
    pub aliases: Vec<String>,
}

#[derive(Default)]
pub(super) struct Declarations {
    pub host: Option<String>,
    pub port: Option<String>,
    pub user: Option<String>,
    pub identities: Vec<String>,
    pub jump: Option<String>,
}

impl ParsedConfig {
    pub fn parse(source: &str) -> SshResult<Self> {
        if source.len() > MAX_CONFIG_BYTES {
            return Err(invalid("config exceeds 1 MiB"));
        }
        let mut entries = Vec::new();
        let mut aliases = Vec::<String>::new();
        for (index, text) in source.trim_start_matches('\u{feff}').lines().enumerate() {
            let line = index + 1;
            if line > 4096 || text.len() > 8192 {
                return Err(invalid(format!(
                    "line {line}: exceeds the 4096-line / 8192-byte line limit"
                )));
            }
            let text = text.trim_start();
            if text.is_empty() || text.starts_with('#') {
                continue;
            }
            let boundary = text
                .find(|c: char| c.is_whitespace() || c == '=')
                .unwrap_or(text.len());
            let key = text[..boundary].to_ascii_lowercase();
            if key.is_empty() || !key.bytes().all(|c| c.is_ascii_alphabetic()) {
                return Err(invalid(format!("line {line}: invalid keyword")));
            }
            let rest = text[boundary..].trim_start();
            let rest = rest.strip_prefix('=').unwrap_or(rest).trim_start();
            let values = words(rest).map_err(|_| {
                invalid(format!("line {line}: invalid quoting or control character"))
            })?;
            if values.is_empty() {
                return Err(invalid(format!("line {line}: {key} requires a value")));
            }
            if key == "host" {
                if values.len() > 256 {
                    return Err(invalid(format!("line {line}: too many Host patterns")));
                }
                for alias in &values {
                    if !alias.contains(['*', '?'])
                        && !alias.starts_with('!')
                        && !aliases.iter().any(|old| old.eq_ignore_ascii_case(alias))
                    {
                        if aliases.len() == 256 {
                            return Err(invalid("config exceeds 256 literal aliases"));
                        }
                        aliases.push(alias.clone());
                    }
                }
            }
            entries.push(Entry { line, key, values });
        }
        Ok(Self { entries, aliases })
    }

    pub fn declarations(&self, alias: &str) -> SshResult<Declarations> {
        let mut result = Declarations::default();
        let mut active = true;
        for entry in &self.entries {
            if entry.key == "match" {
                return Err(invalid(format!(
                    "line {}: Match is unsupported; no condition or command was evaluated",
                    entry.line
                )));
            }
            if entry.key == "host" {
                let positive = entry
                    .values
                    .iter()
                    .any(|p| !p.starts_with('!') && glob(p, alias));
                let negative = entry
                    .values
                    .iter()
                    .any(|p| p.strip_prefix('!').is_some_and(|p| glob(p, alias)));
                active = positive && !negative;
                continue;
            }
            if !active {
                continue;
            }
            if !matches!(
                entry.key.as_str(),
                "hostname" | "port" | "user" | "identityfile" | "proxyjump"
            ) {
                return Err(invalid(format!(
                    "line {}: unsupported {} declaration; no command was executed",
                    entry.line, entry.key
                )));
            }
            if entry.values.len() != 1 {
                return Err(invalid(format!(
                    "line {}: {} requires one value",
                    entry.line, entry.key
                )));
            }
            let value = entry.values[0].clone();
            match entry.key.as_str() {
                "hostname" => {
                    result.host.get_or_insert(value);
                }
                "port" => {
                    result.port.get_or_insert(value);
                }
                "user" => {
                    result.user.get_or_insert(value);
                }
                "proxyjump" => {
                    result.jump.get_or_insert(value);
                }
                "identityfile" => result.identities.push(value),
                _ => unreachable!(),
            }
        }
        Ok(result)
    }
}

fn words(text: &str) -> Result<Vec<String>, ()> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut started = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_control() && c != '\t' {
            return Err(());
        }
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '\\' if chars
                .peek()
                .is_some_and(|next| matches!(next, '"' | '\\' | '#' | ' ' | '\t')) =>
            {
                word.push(chars.next().ok_or(())?);
                started = true;
            }
            '#' if !quoted => break,
            c if c.is_whitespace() && !quoted => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            _ => {
                word.push(c);
                started = true;
            }
        }
    }
    if quoted {
        return Err(());
    }
    if started {
        words.push(word);
    }
    Ok(words)
}

// Greedy * / ? matching with bounded input; no regex, expansion or filesystem access.
fn glob(pattern: &str, value: &str) -> bool {
    let pattern: Vec<_> = pattern.chars().map(|c| c.to_ascii_lowercase()).collect();
    let value: Vec<_> = value.chars().map(|c| c.to_ascii_lowercase()).collect();
    let (mut p, mut v, mut star, mut retry) = (0, 0, None, 0);
    while v < value.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == value[v]) {
            p += 1;
            v += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            p += 1;
            retry = v;
        } else if let Some(s) = star {
            retry += 1;
            v = retry;
            p = s + 1;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}
