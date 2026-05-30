//! A small, dependency-free glob matcher for the `glob` tool.
//!
//! Supports the common subset: `*` (any chars except `/`), `**` (any path
//! segments including `/`), `?` (one char except `/`), and literal text. This
//! covers patterns like `src/**/*.rs`, `*.toml`, `tests/?.txt` without pulling
//! a regex/glob crate.

/// Whether `path` (forward-slashed) matches `pattern`.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = path.chars().collect();
    matches_from(&p, 0, &t, 0)
}

fn matches_from(p: &[char], mut pi: usize, t: &[char], mut ti: usize) -> bool {
    while pi < p.len() {
        match p[pi] {
            '*' => {
                // `**` matches across `/`; single `*` does not.
                let double = pi + 1 < p.len() && p[pi + 1] == '*';
                if double {
                    // Consume `**` (and an optional following `/`).
                    let mut next = pi + 2;
                    if next < p.len() && p[next] == '/' {
                        next += 1;
                    }
                    if next >= p.len() {
                        return true; // trailing ** matches everything
                    }
                    // Try to match the remainder at every position.
                    for k in ti..=t.len() {
                        if matches_from(p, next, t, k) {
                            return true;
                        }
                    }
                    return false;
                } else {
                    // Single `*`: match any run of non-`/` chars.
                    let next = pi + 1;
                    if next >= p.len() {
                        // trailing single * : rest of segment must have no '/'
                        return !t[ti..].contains(&'/');
                    }
                    for k in ti..=t.len() {
                        if k > ti && t[k - 1] == '/' {
                            break; // single * can't cross '/'
                        }
                        if matches_from(p, next, t, k) {
                            return true;
                        }
                    }
                    return false;
                }
            }
            '?' => {
                if ti >= t.len() || t[ti] == '/' {
                    return false;
                }
                pi += 1;
                ti += 1;
            }
            c => {
                if ti >= t.len() || t[ti] != c {
                    return false;
                }
                pi += 1;
                ti += 1;
            }
        }
    }
    ti == t.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_match() {
        assert!(glob_match("src/main.rs", "src/main.rs"));
        assert!(!glob_match("src/main.rs", "src/lib.rs"));
    }

    #[test]
    fn single_star_within_segment() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("src/*.rs", "src/main.rs"));
        // single star does not cross '/'
        assert!(!glob_match("src/*.rs", "src/sub/main.rs"));
    }

    #[test]
    fn double_star_crosses_segments() {
        assert!(glob_match("src/**/*.rs", "src/main.rs"));
        assert!(glob_match("src/**/*.rs", "src/a/b/c.rs"));
        assert!(glob_match("**/*.toml", "deep/nested/cargo.toml"));
        assert!(glob_match("**", "anything/at/all.txt"));
    }

    #[test]
    fn question_mark() {
        assert!(glob_match("?.txt", "a.txt"));
        assert!(!glob_match("?.txt", "ab.txt"));
        assert!(!glob_match("?.txt", "/.txt"));
    }

    #[test]
    fn non_matches() {
        assert!(!glob_match("*.rs", "main.py"));
        assert!(!glob_match("src/**/*.rs", "lib/main.rs"));
    }
}
