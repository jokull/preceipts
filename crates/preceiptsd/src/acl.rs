//! Creating a keychain item whose ACL names our own binaries.
//!
//! The rest of the module writes through `/usr/bin/security`, which is enough
//! to set a trusted-application list and needs no FFI. It is not enough here.
//! A login-keychain item carries a *partition list* as well as an ACL, and the
//! partition is taken from whichever application created the item — so an item
//! created by `/usr/bin/security` is partitioned to Apple's own tools, and our
//! signed binaries can be shut out of an item whose ACL names them. Creating
//! the item ourselves is the only way to be on both lists, and creating it
//! *with* an ACL means one call rather than a second one that would need the
//! keychain password.
//!
//! `security-framework` wraps none of this: it exposes the opaque `SecAccess`
//! type and nothing that builds one. Hence the four declarations below.

use anyhow::{anyhow, Result};
use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::data::CFData;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use core_foundation_sys::array::{kCFTypeArrayCallBacks, CFArrayCreate, CFArrayRef};
use core_foundation_sys::base::{kCFAllocatorDefault, CFRelease, CFTypeRef};
use core_foundation_sys::string::CFStringRef;
use security_framework::os::macos::keychain::SecKeychain;
use core_foundation_sys::base::OSStatus;
use security_framework_sys::base::{errSecSuccess, SecAccessRef};
use security_framework_sys::item::{
    kSecAttrAccount, kSecAttrService, kSecClass, kSecClassGenericPassword, kSecUseKeychain,
    kSecValueData,
};
use security_framework_sys::keychain_item::SecItemAdd;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

pub enum OpaqueSecTrustedApplicationRef {}
type SecTrustedApplicationRef = *mut OpaqueSecTrustedApplicationRef;

#[link(name = "Security", kind = "framework")]
extern "C" {
    /// Reads the *designated requirement* off the binary at `path`. What is
    /// stored is that requirement, not the path — which is why a Developer ID
    /// binary can move, or be rebuilt, and stay trusted.
    fn SecTrustedApplicationCreateFromPath(
        path: *const libc::c_char,
        app: *mut SecTrustedApplicationRef,
    ) -> OSStatus;

    /// An empty `trusted_list` means "no application" — not "any", which is
    /// what `security -A` writes. The two are opposites, so this is only ever
    /// called with a non-empty list.
    fn SecAccessCreate(
        descriptor: CFStringRef,
        trusted_list: CFArrayRef,
        access: *mut SecAccessRef,
    ) -> OSStatus;

    static kSecAttrAccess: CFStringRef;
}

fn os_err(status: OSStatus, what: &str) -> anyhow::Error {
    anyhow!(
        "{what}: {}",
        security_framework::base::Error::from_code(status)
    )
}

/// Add a generic password readable only by `trusted`.
///
/// Fails rather than falling back to an open ACL: quietly downgrading the
/// protection on a secret is worse than not storing it, because the caller
/// would have no way to know which one it got.
pub fn add_generic_password_trusting(
    service: &str,
    account: &str,
    value: &str,
    keychain: Option<&str>,
    trusted: &[std::path::PathBuf],
) -> Result<()> {
    if trusted.is_empty() {
        return Err(anyhow!("refusing to write an ACL that trusts nothing"));
    }
    // Opened before the unsafe block so a bad path is a plain error.
    let opened = match keychain {
        Some(path) => Some(
            SecKeychain::open(path).map_err(|e| anyhow!("cannot open keychain {path}: {e}"))?,
        ),
        None => None,
    };

    let access = build_access(service, trusted)?;

    let mut pairs: Vec<(CFString, CFType)> = unsafe {
        vec![
            (
                CFString::wrap_under_get_rule(kSecClass),
                CFString::wrap_under_get_rule(kSecClassGenericPassword).as_CFType(),
            ),
            (
                CFString::wrap_under_get_rule(kSecAttrService),
                CFString::new(service).as_CFType(),
            ),
            (
                CFString::wrap_under_get_rule(kSecAttrAccount),
                CFString::new(account).as_CFType(),
            ),
            (
                CFString::wrap_under_get_rule(kSecValueData),
                CFData::from_buffer(value.as_bytes()).as_CFType(),
            ),
            (CFString::wrap_under_get_rule(kSecAttrAccess), access),
        ]
    };
    if let Some(keychain) = &opened {
        pairs.push(unsafe {
            (
                CFString::wrap_under_get_rule(kSecUseKeychain),
                keychain.as_CFType(),
            )
        });
    }

    let attributes = CFDictionary::from_CFType_pairs(&pairs);
    let status = unsafe { SecItemAdd(attributes.as_concrete_TypeRef(), ptr::null_mut()) };
    if status != errSecSuccess {
        return Err(os_err(status, format!("storing {account}").as_str()));
    }
    Ok(())
}

/// Build the `SecAccess` for `trusted`, described by `service` so the
/// authorization dialog — if one is ever raised — names the project rather
/// than a hex blob.
fn build_access(service: &str, trusted: &[std::path::PathBuf]) -> Result<CFType> {
    let mut apps: Vec<CFTypeRef> = Vec::with_capacity(trusted.len());
    let result = (|| -> Result<CFType> {
        for path in trusted {
            apps.push(trusted_application(path)? as CFTypeRef);
        }
        let array = unsafe {
            CFArrayCreate(
                kCFAllocatorDefault,
                apps.as_ptr(),
                apps.len() as isize,
                &kCFTypeArrayCallBacks,
            )
        };
        if array.is_null() {
            return Err(anyhow!("could not assemble the trusted-application list"));
        }
        let array = unsafe { CFArray::<CFType>::wrap_under_create_rule(array) };

        let descriptor = CFString::new(service);
        let mut access: SecAccessRef = ptr::null_mut();
        let status = unsafe {
            SecAccessCreate(
                descriptor.as_concrete_TypeRef(),
                array.as_concrete_TypeRef(),
                &mut access,
            )
        };
        if status != errSecSuccess {
            return Err(os_err(status, "building the access list"));
        }
        Ok(unsafe { CFType::wrap_under_create_rule(access as CFTypeRef) })
    })();
    // The array retained each application; these are our own references,
    // owed a release whether or not the rest of it worked.
    for app in apps {
        unsafe { CFRelease(app) };
    }
    result
}

fn trusted_application(path: &Path) -> Result<SecTrustedApplicationRef> {
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| anyhow!("path contains a NUL: {}", path.display()))?;
    let mut app: SecTrustedApplicationRef = ptr::null_mut();
    let status = unsafe { SecTrustedApplicationCreateFromPath(c_path.as_ptr(), &mut app) };
    if status != errSecSuccess {
        return Err(os_err(
            status,
            &format!("reading the signature of {}", path.display()),
        ));
    }
    Ok(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty list is "trust nobody", which would store a secret no process
    /// could ever read — the opposite of the open ACL it might be mistaken
    /// for.
    #[test]
    fn an_empty_trusted_list_is_refused() {
        let err = add_generic_password_trusting("svc", "acct", "v", None, &[]).unwrap_err();
        assert!(err.to_string().contains("trusts nothing"));
    }

    /// Every binary named in an ACL has to be one the system can read a
    /// signature from; a missing path must fail before anything is stored.
    #[test]
    fn a_binary_that_is_not_there_cannot_be_trusted() {
        let missing = std::path::PathBuf::from("/nonexistent/preceipts");
        assert!(trusted_application(&missing).is_err());
    }

    /// The whole mechanism, against a real keychain of its own.
    ///
    /// This test binary is ad-hoc signed, so the requirement it stores is a
    /// hash rather than a team anchor — durability across rebuilds is a
    /// property of Developer ID and cannot be shown here. What *can* be shown
    /// is everything else: that an item is created with an ACL, that the ACL
    /// names the binary we asked for, and that the binary can then read it
    /// back without a prompt. The keychain is created and destroyed by the
    /// test, so nothing of the developer's is touched.
    #[test]
    fn an_item_is_created_naming_the_binary_that_may_read_it() {
        use security_framework::os::macos::passwords::find_generic_password;

        let dir = tempfile::tempdir().expect("tempdir");
        let keychain = dir.path().join("acl-test.keychain-db");
        let path = keychain.to_string_lossy().to_string();
        let created = std::process::Command::new("/usr/bin/security")
            .args(["create-keychain", "-p", "test", &path])
            .output()
            .expect("create-keychain");
        assert!(created.status.success(), "could not create a test keychain");
        let _unlock = std::process::Command::new("/usr/bin/security")
            .args(["unlock-keychain", "-p", "test", &path])
            .output();

        // If anything below would raise a dialog, fail instead of hanging a
        // test run forever behind a window nobody is looking at.
        let _no_prompts = SecKeychain::disable_user_interaction();

        let exe = std::env::current_exe().expect("current_exe");
        let service = "is.solberg.preceipts.acl-test";
        add_generic_password_trusting(service, "TOKEN", "s3cret", Some(&path), &[exe.clone()])
            .expect("writing an item with an ACL");

        let opened = SecKeychain::open(&path).expect("open the test keychain");
        let (password, _item) = find_generic_password(Some(&[opened]), service, "TOKEN")
            .expect("the binary named in the ACL must be able to read it");
        assert_eq!(password.to_vec(), b"s3cret");

        // And the ACL says so, rather than being open to everything.
        let dump = std::process::Command::new("/usr/bin/security")
            .args(["dump-keychain", "-a", &path])
            .output()
            .expect("dump-keychain");
        let text = String::from_utf8_lossy(&dump.stdout);
        assert!(
            text.contains("applications (1)"),
            "expected exactly one trusted application in:\n{text}"
        );

        let _ = std::process::Command::new("/usr/bin/security")
            .args(["delete-keychain", &path])
            .output();
    }
}
