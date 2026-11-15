//! DPAPI helpers — decrypt/encrypt secrets stored in config files with the
//! `enc:` prefix.
//!
//! Must stay byte-compatible with the Python GUI (`restlex.py`,
//! `_dpapi_encrypt`/`_dpapi_decrypt`): `win32crypt.CryptProtectData` with
//! `CRYPTPROTECT_LOCAL_MACHINE`, the blob base64-encoded (RFC 4648, padded) and
//! prefixed with `enc:`. The machine scope lets any process on the host —
//! including this agent running as LocalSystem — decrypt what the GUI wrote in
//! the user session.

/// Prefix marking a value as DPAPI-encrypted + base64 in the config JSON.
pub const ENC_PREFIX: &str = "enc:";

/// True if the value still carries the `enc:` prefix, i.e. it was NOT decrypted
/// (wrong machine / reinstalled OS / corrupt blob). Such a value is a ciphertext
/// blob and MUST NOT be used as a password, API key or bot token — check this
/// before handing a secret to restic, an HTTP header or Telegram.
pub fn is_undecrypted(s: &str) -> bool {
    s.starts_with(ENC_PREFIX)
}

/// Decrypt a possibly-`enc:`-prefixed value. Three outcomes:
/// * no `enc:` prefix — returned unchanged (legacy/plaintext configs);
/// * decrypted — plaintext;
/// * decrypt failed — the original `enc:` blob is returned *deliberately* so a
///   read-modify-write of the config does not wipe the secret from disk, and an
///   error is logged (без самого значения). Callers that USE the secret must
///   reject it via [`is_undecrypted`] — better [`decrypt_secret_checked`], which
///   returns `None` in this case.
pub fn decrypt_secret(s: &str) -> String {
    decrypt_secret_named(s, "secret")
}

/// Like [`decrypt_secret`], but names the key in the failure log.
pub fn decrypt_secret_named(s: &str, key_name: &str) -> String {
    #[cfg(windows)]
    {
        if let Some(b64) = s.strip_prefix(ENC_PREFIX) {
            if let Some(plain) = imp::unprotect(b64) {
                return plain;
            }
            tracing::error!("DPAPI decrypt failed для {key_name}");
            // Leave as-is so a subsequent write re-emits the same encrypted
            // value, not garbage. Consumers must check `is_undecrypted`.
            return s.to_string();
        }
        s.to_string()
    }
    #[cfg(not(windows))]
    {
        let _ = key_name;
        s.to_string()
    }
}

/// Decrypt for USE (not for write-back): `None` when the value is still an
/// undecryptable `enc:` blob, so the ciphertext can never travel as a password
/// or an `X-Api-Key` header.
pub fn decrypt_secret_checked(s: &str, key_name: &str) -> Option<String> {
    let v = decrypt_secret_named(s, key_name);
    if is_undecrypted(&v) {
        None
    } else {
        Some(v)
    }
}

/// Encrypt a plaintext secret to the `enc:` form for storage. Empty values and
/// values already carrying the `enc:` prefix are returned unchanged (idempotent;
/// safe to call on a value that survived a failed decrypt). If DPAPI is
/// unavailable the plaintext is returned, mirroring the Python fallback.
pub fn encrypt_secret(s: &str) -> String {
    #[cfg(windows)]
    {
        if s.is_empty() || s.starts_with(ENC_PREFIX) {
            return s.to_string();
        }
        match imp::protect(s) {
            Some(b64) => format!("{ENC_PREFIX}{b64}"),
            None => s.to_string(),
        }
    }
    #[cfg(not(windows))]
    {
        s.to_string()
    }
}

#[cfg(windows)]
mod imp {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use core::ffi::c_void;

    #[repr(C)]
    struct DataBlob {
        cb_data: u32,
        pb_data: *mut u8,
    }

    // wincrypt.h
    const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;
    const CRYPTPROTECT_LOCAL_MACHINE: u32 = 0x4;

    #[link(name = "crypt32")]
    extern "system" {
        fn CryptProtectData(
            p_data_in: *const DataBlob,
            sz_data_descr: *const u16,
            p_optional_entropy: *const DataBlob,
            p_reserved: *mut c_void,
            p_prompt_struct: *mut c_void,
            dw_flags: u32,
            p_data_out: *mut DataBlob,
        ) -> i32;
        fn CryptUnprotectData(
            p_data_in: *const DataBlob,
            pp_sz_data_descr: *mut *mut u16,
            p_optional_entropy: *const DataBlob,
            p_reserved: *mut c_void,
            p_prompt_struct: *mut c_void,
            dw_flags: u32,
            p_data_out: *mut DataBlob,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(h_mem: *mut c_void) -> *mut c_void;
    }

    pub fn protect(plain: &str) -> Option<String> {
        let mut input = plain.as_bytes().to_vec();
        let in_blob = DataBlob {
            cb_data: input.len() as u32,
            pb_data: input.as_mut_ptr(),
        };
        let mut out = DataBlob {
            cb_data: 0,
            pb_data: core::ptr::null_mut(),
        };
        // SAFETY: in_blob points at `input`'s live buffer; CryptProtectData
        // allocates out.pb_data via LocalAlloc, which we free with LocalFree.
        let ok = unsafe {
            CryptProtectData(
                &in_blob,
                core::ptr::null(),
                core::ptr::null(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                CRYPTPROTECT_LOCAL_MACHINE | CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 || out.pb_data.is_null() {
            return None;
        }
        let b64 = unsafe {
            let slice = core::slice::from_raw_parts(out.pb_data, out.cb_data as usize);
            let s = STANDARD.encode(slice);
            LocalFree(out.pb_data as *mut c_void);
            s
        };
        Some(b64)
    }

    pub fn unprotect(b64: &str) -> Option<String> {
        let mut data = STANDARD.decode(b64.trim()).ok()?;
        let in_blob = DataBlob {
            cb_data: data.len() as u32,
            pb_data: data.as_mut_ptr(),
        };
        let mut out = DataBlob {
            cb_data: 0,
            pb_data: core::ptr::null_mut(),
        };
        // SAFETY: same contract as protect(); out buffer freed with LocalFree.
        let ok = unsafe {
            CryptUnprotectData(
                &in_blob,
                core::ptr::null_mut(),
                core::ptr::null(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                CRYPTPROTECT_LOCAL_MACHINE | CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 || out.pb_data.is_null() {
            return None;
        }
        let plain = unsafe {
            let slice = core::slice::from_raw_parts(out.pb_data, out.cb_data as usize);
            let s = String::from_utf8_lossy(slice).into_owned();
            LocalFree(out.pb_data as *mut c_void);
            s
        };
        Some(plain)
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_local_machine() {
        let secret = "S3cr3t-пароль!";
        let enc = encrypt_secret(secret);
        assert!(enc.starts_with(ENC_PREFIX), "expected enc: prefix, got {enc}");
        assert_eq!(decrypt_secret(&enc), secret);
    }

    #[test]
    fn plaintext_passthrough() {
        assert_eq!(decrypt_secret("plain"), "plain");
        assert_eq!(decrypt_secret(""), "");
    }

    #[test]
    fn undecryptable_blob_is_never_returned_as_secret() {
        let broken = "enc:not-a-valid-base64-blob!!!";
        // Для использования — None, шифротекст не уйдёт в RESTIC_PASSWORD/X-Api-Key.
        assert!(decrypt_secret_checked(broken, "repo_password").is_none());
        // Для write-back значение сохраняется, но помечено как нерасшифрованное.
        assert!(is_undecrypted(&decrypt_secret(broken)));
        assert!(!is_undecrypted("plain-password"));
        assert_eq!(decrypt_secret_checked("plain", "x").as_deref(), Some("plain"));
    }

    #[test]
    fn encrypt_is_idempotent_on_enc_and_empty() {
        let enc = encrypt_secret("abc");
        assert_eq!(encrypt_secret(&enc), enc, "must not double-encrypt");
        assert_eq!(encrypt_secret(""), "");
    }
}
