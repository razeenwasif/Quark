//! A PDF's passwords: one to open it, one to change what it allows — AES-256,
//! the standard's strongest — and what it allows: printing, copying,
//! changing. A file opened with its password is saved under the same keys
//! unless it is given new ones.

use std::collections::BTreeMap;
use std::sync::Arc;

use lopdf::encryption::crypt_filters::{Aes256CryptFilter, CryptFilter};
use lopdf::{Document, EncryptionState, EncryptionVersion, Object, Permissions};
use serde::{Deserialize, Serialize};

/// A password, as it crosses the bus: itself to serde, dots to `Debug`, so
/// no trace or log of a command ever shows it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("\"•••\"")
    }
}

/// Passwords to save a PDF with, and what someone with only the first may do.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Protection {
    /// To open it; empty for anyone.
    pub user: String,
    /// To change what it allows; the first, when empty.
    pub owner: String,
    pub print: bool,
    pub copy: bool,
    pub edit: bool,
}

impl std::fmt::Debug for Protection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // What it allows, never what it is opened with.
        f.debug_struct("Protection")
            .field("print", &self.print)
            .field("copy", &self.copy)
            .field("edit", &self.edit)
            .finish_non_exhaustive()
    }
}

/// What a protected PDF allows, as its permissions say.
pub fn allowed(p: &Protection) -> Permissions {
    let mut perms = Permissions::COPYABLE_FOR_ACCESSIBILITY;
    if p.print {
        perms |= Permissions::PRINTABLE | Permissions::PRINTABLE_IN_HIGH_QUALITY;
    }
    if p.copy {
        perms |= Permissions::COPYABLE;
    }
    if p.edit {
        perms |= Permissions::MODIFIABLE
            | Permissions::ANNOTABLE
            | Permissions::FILLABLE
            | Permissions::ASSEMBLABLE;
    }
    perms
}

/// `doc` — decrypted — encrypted with AES-256 under these passwords.
pub fn protect(doc: &mut Document, p: &Protection) -> Result<(), String> {
    if p.user.is_empty() && p.owner.is_empty() {
        return Err("give it a password".into());
    }
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(|e| format!("no random key could be made: {e}"))?;
    let filter: Arc<dyn CryptFilter> = Arc::new(Aes256CryptFilter);
    let owner = if p.owner.is_empty() {
        p.user.as_str()
    } else {
        p.owner.as_str()
    };
    let state = EncryptionState::try_from(EncryptionVersion::V5 {
        encrypt_metadata: true,
        crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), filter)]),
        file_encryption_key: &key,
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password: owner,
        user_password: &p.user,
        permissions: allowed(p),
    })
    .map_err(|e| format!("the passwords could not be set: {e}"))?;
    reencrypt(doc, &state)
}

/// `doc` — decrypted — encrypted again under a state it was read with.
pub fn reencrypt(doc: &mut Document, state: &EncryptionState) -> Result<(), String> {
    unprotect(doc);
    doc.encrypt(state)
        .map_err(|e| format!("it could not be encrypted: {e}"))
}

/// A decrypted document's leftover encryption dictionary taken away, so it
/// is written plain.
pub fn unprotect(doc: &mut Document) {
    if let Ok(Object::Reference(id)) = doc.trailer.get(b"Encrypt").cloned() {
        doc.objects.remove(&id);
    }
    doc.trailer.remove(b"Encrypt");
    doc.encryption_state = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pdf_is_protected_opened_only_with_its_password_and_keeps_what_it_says() {
        let mut doc = Document::load_mem(&crate::fixture::fixture(&["Secret words."])).unwrap();
        protect(
            &mut doc,
            &Protection {
                user: "open sesame".into(),
                owner: "boss".into(),
                print: true,
                copy: false,
                edit: false,
            },
        )
        .unwrap();
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains("Secret words"),
            "the words are not there to read"
        );
        assert!(Document::load_mem_with_options(
            &bytes,
            lopdf::LoadOptions::with_password("wrong")
        )
        .is_err());
        let opened = Document::load_mem_with_options(
            &bytes,
            lopdf::LoadOptions::with_password("open sesame"),
        )
        .unwrap();
        assert!(opened.was_encrypted());
        let text =
            String::from_utf8_lossy(&opened.get_page_content(opened.get_pages()[&1])).to_string();
        assert!(text.contains("Secret words"), "{text}");
        assert!(protect(
            &mut doc.clone(),
            &Protection {
                user: String::new(),
                owner: String::new(),
                print: true,
                copy: true,
                edit: true
            }
        )
        .is_err());
    }

    #[test]
    fn a_password_is_never_shown_by_debug() {
        let p = Protection {
            user: "hunter2".into(),
            owner: "boss".into(),
            print: true,
            copy: false,
            edit: false,
        };
        let shown = format!("{p:?} {:?}", Secret("hunter2".into()));
        assert!(
            !shown.contains("hunter2") && !shown.contains("boss"),
            "{shown}"
        );
        assert_eq!(serde_json::to_string(&Secret("x".into())).unwrap(), "\"x\"");
    }
}
