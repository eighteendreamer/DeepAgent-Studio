use super::{
    invalid,
    parser::{Declarations, ParsedConfig},
    SshConfigImportedConnection, SshConfigImportedEndpoint, SshConfigOverrides, SshResult,
};
use crate::SshHostIdentity;
use std::path::{Path, PathBuf};

pub(super) fn resolve(
    parsed: &ParsedConfig,
    home: &Path,
    local_user: &str,
    alias: &str,
    overrides: &SshConfigOverrides,
) -> SshResult<SshConfigImportedConnection> {
    SshHostIdentity::direct(alias, 22)?;
    if alias.contains(['*', '?', '!', '%', '$']) {
        return Err(invalid("invalid target alias"));
    }
    let mut declarations = parsed.declarations(alias)?;
    if let Some(value) = &overrides.host {
        declarations.host = Some(value.clone());
    }
    if let Some(value) = overrides.port {
        declarations.port = Some(value.to_string());
    }
    if let Some(value) = &overrides.username {
        declarations.user = Some(value.clone());
    }
    if let Some(value) = &overrides.identity_files {
        if value.len() > crate::MAX_AUTHENTICATION_FACTORS {
            return Err(invalid("identity override exceeds six explicit files"));
        }
        declarations.identities = value.clone();
        if value.is_empty() {
            declarations.identities.push("none".into());
        }
    }
    let jump_spec = overrides
        .proxy_jump
        .as_ref()
        .or(declarations.jump.as_ref())
        .cloned();
    let target = endpoint(&declarations, home, local_user, alias)?;
    let jump = jump_spec
        .filter(|value| !value.eq_ignore_ascii_case("none"))
        .map(|spec| jump_endpoint(parsed, home, local_user, &spec))
        .transpose()?;
    if jump
        .as_ref()
        .is_some_and(|jump| jump.host == target.host && jump.port == target.port)
    {
        return Err(invalid("ProxyJump resolves to the target itself"));
    }
    Ok(SshConfigImportedConnection {
        alias: alias.into(),
        target,
        jump,
    })
}

fn endpoint(
    declarations: &Declarations,
    home: &Path,
    local_user: &str,
    alias: &str,
) -> SshResult<SshConfigImportedEndpoint> {
    let raw_host = declarations.host.as_deref().unwrap_or(alias);
    let host = expand_hostname(raw_host, alias)?;
    let port = declarations
        .port
        .as_deref()
        .unwrap_or("22")
        .parse::<u16>()
        .map_err(|_| invalid("Port must be between 1 and 65535"))?;
    let host = SshHostIdentity::direct(&host, port)?;
    let username = declarations
        .user
        .as_deref()
        .unwrap_or(local_user)
        .to_owned();
    if username.is_empty()
        || username.len() > 255
        || username
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(invalid(
            "User is required and must contain no whitespace or control characters",
        ));
    }
    let mut endpoint = SshConfigImportedEndpoint {
        host: host.host,
        port,
        username,
        identity_files: Vec::new(),
        default_identities: declarations.identities.is_empty(),
    };
    let none = declarations
        .identities
        .iter()
        .any(|value| value.eq_ignore_ascii_case("none"));
    if none
        && declarations
            .identities
            .iter()
            .any(|value| !value.eq_ignore_ascii_case("none"))
    {
        return Err(invalid(
            "IdentityFile none combined with other files is unsupported",
        ));
    }
    if !none {
        for value in &declarations.identities {
            let path = identity_path(value, home, alias, &endpoint)?;
            if !endpoint.identity_files.contains(&path) {
                endpoint.identity_files.push(path);
            }
            if endpoint.identity_files.len() > crate::MAX_AUTHENTICATION_FACTORS {
                return Err(invalid(
                    "IdentityFile exceeds the six explicit identity limit",
                ));
            }
        }
    }
    Ok(endpoint)
}

fn expand_hostname(value: &str, alias: &str) -> SshResult<String> {
    let mut result = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            match chars.next() {
                Some('h') => result.push_str(alias),
                Some('%') => result.push('%'),
                _ => return Err(invalid("unsupported HostName token")),
            }
        } else {
            result.push(c);
        }
    }
    if result.contains(['$', '%']) {
        return Err(invalid("unsupported HostName expansion"));
    }
    Ok(result)
}

fn identity_path(
    value: &str,
    home: &Path,
    alias: &str,
    endpoint: &SshConfigImportedEndpoint,
) -> SshResult<String> {
    if value.is_empty()
        || value.len() > 8192
        || value.contains('$')
        || value.chars().any(char::is_control)
    {
        return Err(invalid("empty or unsupported IdentityFile expansion"));
    }
    let home_text = home
        .to_str()
        .ok_or_else(|| invalid("home path is not Unicode"))?;
    let mut expanded = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            expanded.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => expanded.push('%'),
            Some('d') => expanded.push_str(home_text),
            Some('h') => expanded.push_str(&endpoint.host),
            Some('n') => expanded.push_str(alias),
            Some('p') => expanded.push_str(&endpoint.port.to_string()),
            Some('r') => expanded.push_str(&endpoint.username),
            _ => return Err(invalid("unsupported IdentityFile token")),
        }
    }
    let path = if let Some(rest) = expanded
        .strip_prefix("~/")
        .or_else(|| expanded.strip_prefix("~\\"))
    {
        home.join(rest)
    } else if expanded.starts_with('~') {
        return Err(invalid(
            "named-user IdentityFile tilde expansion is unsupported",
        ));
    } else {
        PathBuf::from(expanded)
    };
    if !path.is_absolute() {
        return Err(invalid(
            "relative IdentityFile requires an explicit absolute-path override",
        ));
    }
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid("IdentityFile path is not Unicode"))
}

fn jump_endpoint(
    parsed: &ParsedConfig,
    home: &Path,
    local_user: &str,
    spec: &str,
) -> SshResult<SshConfigImportedEndpoint> {
    if spec.contains(',') || spec.contains("//") || spec.chars().any(char::is_whitespace) {
        return Err(invalid("only one ProxyJump hop is supported"));
    }
    let (user, authority) = match spec.rsplit_once('@') {
        Some((user, host)) => (Some(user), host),
        None => (None, spec),
    };
    let (alias, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, suffix) = rest
            .split_once(']')
            .ok_or_else(|| invalid("invalid ProxyJump IPv6 endpoint"))?;
        let port = if suffix.is_empty() {
            None
        } else {
            Some(
                suffix
                    .strip_prefix(':')
                    .ok_or_else(|| invalid("invalid ProxyJump port"))?,
            )
        };
        (host, port)
    } else if let Some((host, port)) = authority.split_once(':') {
        if port.contains(':') {
            return Err(invalid("ProxyJump IPv6 must be bracketed"));
        }
        (host, Some(port))
    } else {
        (authority, None)
    };
    SshHostIdentity::direct(alias, 22)?;
    let mut declarations = parsed.declarations(alias)?;
    if declarations
        .jump
        .as_ref()
        .is_some_and(|jump| !jump.eq_ignore_ascii_case("none"))
    {
        return Err(invalid("nested or cyclic ProxyJump is unsupported"));
    }
    if let Some(user) = user {
        declarations.user = Some(user.into());
    }
    if let Some(port) = port {
        declarations.port = Some(port.into());
    }
    endpoint(&declarations, home, local_user, alias)
}
