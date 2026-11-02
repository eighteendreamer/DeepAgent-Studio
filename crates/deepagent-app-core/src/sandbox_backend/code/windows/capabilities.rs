//! Compare the actual suspended token with the declared capability set.
//! Node's only named capability is registryRead; network grants are absent.
use super::{checked, os_error, wide};
use deepagent_core::error::{CoreError, Result};
use deepagent_runtime::workflow::code::CodeLanguage;
use std::mem::size_of;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{LocalFree, HANDLE};
use windows_sys::Win32::Security::{
    DeriveCapabilitySidsFromName, GetTokenInformation, TokenCapabilities, PSID, SID_AND_ATTRIBUTES,
    TOKEN_GROUPS,
};

type Capability = (String, u32);

// Documented SE_GROUP_ENABLED; avoid another bindings feature for this value.
// https://learn.microsoft.com/windows/win32/api/winnt/ns-winnt-token_groups
const ENABLED_CAPABILITY: u32 = 4;
// Independent oracle, not an arbitrary caller-supplied capability name/SID.
// https://chromium.googlesource.com/chromium/src/+/8073c7e0afcb/base/win/sid_unittest.cc
const REGISTRY_READ_SID: &str = "S-1-15-3-1024-1065365936-1281604716-3511738428-1654721687-432734479-3232135806-4053264122-3456934681";

#[derive(Default)]
struct SidArray {
    pointer: *mut PSID,
    count: u32,
}
impl Drop for SidArray {
    fn drop(&mut self) {
        // Each returned SID AND the array have LocalFree ownership:
        // https://learn.microsoft.com/windows/win32/api/securitybaseapi/nf-securitybaseapi-derivecapabilitysidsfromname
        if !self.pointer.is_null() {
            unsafe {
                for index in 0..self.count as usize {
                    LocalFree(*self.pointer.add(index));
                }
                LocalFree(self.pointer.cast());
            }
        }
    }
}

pub(super) struct RuntimeCapabilities {
    _sids: SidArray,
    entries: Vec<SID_AND_ATTRIBUTES>,
}
impl RuntimeCapabilities {
    pub(super) fn for_language(language: CodeLanguage) -> Result<Self> {
        // The authorized and tested named capability is Node-specific. Do not
        // silently extend another runtime's OS resource grants.
        if language != CodeLanguage::Javascript {
            return Ok(Self {
                _sids: SidArray::default(),
                entries: Vec::new(),
            });
        }
        let name = wide("registryRead")?;
        let mut groups = SidArray::default();
        let mut sids = SidArray::default();
        checked(
            unsafe {
                DeriveCapabilitySidsFromName(
                    name.as_ptr(),
                    &mut groups.pointer,
                    &mut groups.count,
                    &mut sids.pointer,
                    &mut sids.count,
                )
            },
            "derive Node runtime capability",
        )?;
        if sids.count != 1 || sids.pointer.is_null() {
            return Err(CoreError::other(
                "CodeIsolationFailed: unexpected derived runtime capability count",
            ));
        }
        let sid = unsafe { *sids.pointer };
        if super::profile::sid_string(sid)? != REGISTRY_READ_SID {
            return Err(CoreError::other(
                "CodeIsolationFailed: unexpected derived runtime capability SID",
            ));
        }
        Ok(Self {
            _sids: sids,
            entries: vec![SID_AND_ATTRIBUTES {
                Sid: sid,
                Attributes: ENABLED_CAPABILITY,
            }],
        })
    }

    pub(super) fn entries(&self) -> &[SID_AND_ATTRIBUTES] {
        &self.entries
    }
    pub(super) fn entries_mut(&mut self) -> &mut [SID_AND_ATTRIBUTES] {
        &mut self.entries
    }
}

fn identities(entries: &[SID_AND_ATTRIBUTES]) -> Result<Vec<Capability>> {
    entries
        .iter()
        .map(|entry| Ok((super::profile::sid_string(entry.Sid)?, entry.Attributes)))
        .collect()
}

fn exact_set(mut actual: Vec<Capability>, mut expected: Vec<Capability>) -> bool {
    actual.sort_unstable();
    expected.sort_unstable();
    // Reject duplicate declarations/observations as well as extra capabilities.
    actual == expected && !actual.windows(2).any(|pair| pair[0].0 == pair[1].0)
}

pub(super) fn verify(token: HANDLE, expected: &[SID_AND_ATTRIBUTES]) -> Result<()> {
    let mut length = 0;
    unsafe { GetTokenInformation(token, TokenCapabilities, null_mut(), 0, &mut length) };
    if length < size_of::<u32>() as u32 {
        return Err(os_error("size SDK capabilities"));
    }
    let mut storage = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
    checked(
        unsafe {
            GetTokenInformation(
                token,
                TokenCapabilities,
                storage.as_mut_ptr().cast(),
                (storage.len() * size_of::<usize>()) as u32,
                &mut length,
            )
        },
        "verify SDK capabilities",
    )?;
    if length < size_of::<u32>() as u32 || length as usize > storage.len() * size_of::<usize>() {
        return Err(CoreError::other(
            "CodeIsolationFailed: truncated SDK capabilities",
        ));
    }
    let count = unsafe { *storage.as_ptr().cast::<u32>() } as usize;
    let actual = if count == 0 {
        Vec::new()
    } else {
        // TOKEN_GROUPS is variable-sized. Validate the entry array before
        // creating a slice; SID pointers remain owned by this token buffer.
        let groups = storage.as_ptr().cast::<TOKEN_GROUPS>();
        let entries = unsafe { std::ptr::addr_of!((*groups).Groups).cast::<SID_AND_ATTRIBUTES>() };
        let offset = entries as usize - storage.as_ptr() as usize;
        let available = (length as usize).saturating_sub(offset) / size_of::<SID_AND_ATTRIBUTES>();
        if count > available {
            return Err(CoreError::other(
                "CodeIsolationFailed: truncated SDK capabilities",
            ));
        }
        identities(unsafe { std::slice::from_raw_parts(entries, count) })?
    };
    if !exact_set(actual, identities(expected)?) {
        return Err(CoreError::other(
            "CodeIsolationFailed: unexpected SDK capability",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_capabilities_are_exact_named_node_read_only_and_no_other_runtime_grants() {
        let node = RuntimeCapabilities::for_language(CodeLanguage::Javascript).unwrap();
        assert_eq!(
            identities(node.entries()).unwrap(),
            vec![(REGISTRY_READ_SID.into(), ENABLED_CAPABILITY)]
        );
        let python = RuntimeCapabilities::for_language(CodeLanguage::Python).unwrap();
        assert!(python.entries().is_empty());
    }

    #[test]
    fn exact_capabilities_reject_extras_missing_attributes_and_duplicates() {
        let registry = ("registry-fixture".to_owned(), 4);
        let unknown = ("unknown-fixture".to_owned(), 4);
        assert!(exact_set(vec![], vec![]));
        assert!(exact_set(vec![registry.clone()], vec![registry.clone()]));
        assert!(!exact_set(vec![registry.clone()], vec![]));
        assert!(!exact_set(vec![], vec![registry.clone()]));
        assert!(!exact_set(vec![unknown.clone()], vec![registry.clone()]));
        assert!(!exact_set(
            vec![(registry.0.clone(), 0)],
            vec![registry.clone()]
        ));
        assert!(!exact_set(
            vec![registry.clone(), unknown],
            vec![registry.clone()]
        ));
        assert!(!exact_set(
            vec![registry.clone(), registry.clone()],
            vec![registry.clone(), registry]
        ));
    }
}
