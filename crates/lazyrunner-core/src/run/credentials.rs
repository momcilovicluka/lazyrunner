//! The passwords of database sources (#110), kept in the OS's credential store: Windows Credential Manager, under
//! `lazyrunner/<project>/<login>@<server>` - one password for every source with that server and login (#114); one
//! kept by 1.118.0 under `lazyrunner/<project>/<source>` is still found, and moved there quietly. They're never
//! written to the profile, the config, a log or an answer to an agent: a password goes from the store to sqlcmd's
//! environment and SqlPackage's response file, and nowhere else. Other OSes' stores aren't supported yet: there, the
//! password is asked for each time.

/// Where 1.118.0 kept a source's password: `lazyrunner/<project>/<source>`.
pub fn target(project: &str, source: &str) -> String {
    format!("lazyrunner/{project}/{source}")
}

/// Where a server and login's password is kept: `lazyrunner/<project>/<login>@<server>` (a source's name never has
/// an `@`, so the two never meet).
pub fn shared_target(project: &str, login: &str) -> String {
    format!("lazyrunner/{project}/{login}")
}

/// A password: what it holds is read with [`Password::expose`] only, and never shown by `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct Password(String);

impl Password {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Password {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Password(***)")
    }
}

/// Where passwords are kept: the OS's credential store, or the tests' stand-in.
pub trait Store: Send + Sync {
    /// The password kept at `target`, if one is.
    fn get(&self, target: &str) -> Result<Option<Password>, String>;
    /// Keeps `password` (`user`'s) at `target`, over the one there.
    fn set(&self, target: &str, user: &str, password: &Password) -> Result<(), String>;
    /// Forgets the password at `target`; whether one was there.
    fn forget(&self, target: &str) -> Result<bool, String>;
    /// What the store is called, as lazyrunner says where a password went: `Credential Manager`.
    fn name(&self) -> &'static str;
}

/// The OS's credential store.
pub fn system() -> std::sync::Arc<dyn Store> {
    #[cfg(windows)]
    {
        std::sync::Arc::new(windows::CredentialManager)
    }
    #[cfg(not(windows))]
    {
        std::sync::Arc::new(Unsupported)
    }
}

/// An OS whose store lazyrunner can't use yet: nothing is kept, the password is asked for each time.
pub struct Unsupported;

/// What lazyrunner says where it can't keep a password.
pub const UNSUPPORTED: &str = "this OS's credential store isn't supported yet - the password is asked for each time";

impl Store for Unsupported {
    fn get(&self, _: &str) -> Result<Option<Password>, String> {
        Ok(None)
    }

    fn set(&self, _: &str, _: &str, _: &Password) -> Result<(), String> {
        Err(UNSUPPORTED.into())
    }

    fn forget(&self, _: &str) -> Result<bool, String> {
        Ok(false)
    }

    fn name(&self) -> &'static str {
        "nowhere"
    }
}

/// The tests' store: in memory.
#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
pub struct Memory(std::sync::Mutex<std::collections::HashMap<String, (String, Password)>>);

#[cfg(any(test, feature = "test-support"))]
impl Store for Memory {
    fn get(&self, target: &str) -> Result<Option<Password>, String> {
        let kept = self.0.lock().map_err(|error| error.to_string())?;
        Ok(kept.get(target).map(|(_, password)| password.clone()))
    }

    fn set(&self, target: &str, user: &str, password: &Password) -> Result<(), String> {
        let mut kept = self.0.lock().map_err(|error| error.to_string())?;
        kept.insert(target.to_string(), (user.to_string(), password.clone()));
        Ok(())
    }

    fn forget(&self, target: &str) -> Result<bool, String> {
        let mut kept = self.0.lock().map_err(|error| error.to_string())?;
        Ok(kept.remove(target).is_some())
    }

    fn name(&self) -> &'static str {
        "the tests' store"
    }
}

#[cfg(windows)]
mod windows {
    //! Windows Credential Manager: a generic credential per source, its password the blob (UTF-16), kept on this
    //! machine for this user.

    use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, FILETIME, GetLastError};
    use windows_sys::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW, CredWriteW,
    };

    use super::{Password, Store};

    /// The store's own name, as Windows calls it.
    const NAME: &str = "Credential Manager";
    /// The most a generic credential's blob holds.
    const BLOB_MOST: usize = 5 * 512;

    pub struct CredentialManager;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn failed(what: &str) -> String {
        // SAFETY: reads this thread's last error, set by the call just made.
        let code = unsafe { GetLastError() };
        format!("{NAME} couldn't {what} the password (error {code})")
    }

    impl Store for CredentialManager {
        fn get(&self, target: &str) -> Result<Option<Password>, String> {
            let name = wide(target);
            let mut found: *mut CREDENTIALW = std::ptr::null_mut();
            // SAFETY: `name` is a NUL-terminated UTF-16 string; on success `found` points at a credential Windows
            // allocated, freed below with CredFree.
            let read = unsafe { CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut found) };
            if read == 0 {
                // SAFETY: as in `failed`.
                if unsafe { GetLastError() } == ERROR_NOT_FOUND {
                    return Ok(None);
                }
                return Err(failed("read"));
            }
            // SAFETY: `found` is the credential CredReadW gave; its blob is `CredentialBlobSize` bytes.
            let password = unsafe {
                let credential = &*found;
                let bytes =
                    std::slice::from_raw_parts(credential.CredentialBlob, credential.CredentialBlobSize as usize);
                let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
                CredFree(found.cast());
                String::from_utf16_lossy(&units)
            };
            Ok(Some(Password::new(password)))
        }

        fn set(&self, target: &str, user: &str, password: &Password) -> Result<(), String> {
            let mut blob: Vec<u8> = password.expose().encode_utf16().flat_map(u16::to_le_bytes).collect();
            if blob.len() > BLOB_MOST {
                return Err(format!("the password is longer than {NAME} keeps"));
            }
            let (mut name, mut login) = (wide(target), wide(user));
            let credential = CREDENTIALW {
                Flags: 0,
                Type: CRED_TYPE_GENERIC,
                TargetName: name.as_mut_ptr(),
                Comment: std::ptr::null_mut(),
                LastWritten: FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 },
                CredentialBlobSize: blob.len() as u32,
                CredentialBlob: blob.as_mut_ptr(),
                Persist: CRED_PERSIST_LOCAL_MACHINE,
                AttributeCount: 0,
                Attributes: std::ptr::null_mut(),
                TargetAlias: std::ptr::null_mut(),
                UserName: login.as_mut_ptr(),
            };
            // SAFETY: every pointer in `credential` points at a buffer that outlives the call.
            let written = unsafe { CredWriteW(&credential, 0) };
            blob.fill(0);
            if written == 0 { Err(failed("keep")) } else { Ok(()) }
        }

        fn forget(&self, target: &str) -> Result<bool, String> {
            let name = wide(target);
            // SAFETY: `name` is a NUL-terminated UTF-16 string.
            let deleted = unsafe { CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) };
            if deleted != 0 {
                return Ok(true);
            }
            // SAFETY: as in `failed`.
            if unsafe { GetLastError() } == ERROR_NOT_FOUND { Ok(false) } else { Err(failed("forget")) }
        }

        fn name(&self) -> &'static str {
            NAME
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_is_kept_by_its_source_and_never_shown() {
        assert_eq!(target("shop", "qa"), "lazyrunner/shop/qa");
        assert_eq!(shared_target("shop", "reader@qa-sql"), "lazyrunner/shop/reader@qa-sql");
        let password = Password::new("s3cret");
        assert_eq!(format!("{password:?}"), "Password(***)");
        let store = Memory::default();
        assert_eq!(store.get("lazyrunner/shop/qa"), Ok(None));
        store.set("lazyrunner/shop/qa", "reader", &password).unwrap();
        assert_eq!(store.get("lazyrunner/shop/qa").unwrap().unwrap().expose(), "s3cret");
        assert_eq!(store.forget("lazyrunner/shop/qa"), Ok(true));
        assert_eq!(store.forget("lazyrunner/shop/qa"), Ok(false));
        assert!(Unsupported.set("x", "y", &password).is_err() && Unsupported.get("x") == Ok(None));
    }

    /// The real store, under a target of its own that it deletes after.
    #[cfg(windows)]
    #[test]
    fn credential_manager_keeps_reads_and_forgets_a_password() {
        let store = windows::CredentialManager;
        let target = format!("lazyrunner-selftest/{}/qa", std::process::id());
        let password = Password::new("p;a\"ss wörd");
        store.set(&target, "reader", &password).unwrap();
        let read = store.get(&target);
        let forgot = store.forget(&target);
        assert_eq!(read.unwrap().as_ref().map(Password::expose), Some("p;a\"ss wörd"));
        assert_eq!(forgot, Ok(true));
        assert_eq!(store.get(&target), Ok(None));
    }
}
