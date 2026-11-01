//! One opened APK: its DEX entries, inflated on first use, and the class lookups that decide
//! which mini DEX files JADX gets for a target class.

use std::collections::{BTreeSet, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use rust_ASC::asc_core::core::dex::dex_manager::DexManager;
use rust_ASC::asc_core::utils::tinydex::DEX;
use sha1::{Digest, Sha1};

use crate::class_name::ClassName;
use crate::error::{Error, Result};

const NO_INDEX: u32 = 0xFFFF_FFFF;

/// A mini DEX holding one class, ready for JADX.
pub struct MiniDex {
    pub class: ClassName,
    pub bytes: Vec<u8>,
}

struct DexEntry {
    name: String,
    /// One manager per DEX, reused across extracts: it caches the decoded string pool.
    manager: Option<DexManager>,
    /// Every class descriptor defined in this DEX, in class_defs order.
    descriptors: Option<Vec<ClassName>>,
}

pub struct ApkSession {
    zip: zip::ZipArchive<File>,
    entries: Vec<DexEntry>,
    /// Classes that no entry defines. Each miss otherwise costs a scan of every DEX type table.
    missing: HashSet<ClassName>,
}

impl ApkSession {
    pub fn open(path: &Path) -> Result<ApkSession> {
        let open_err = |e: &dyn std::fmt::Display| {
            Error::Apk(format!("cannot open APK {}: {e}", path.display()))
        };
        let file = File::open(path).map_err(|e| open_err(&e))?;
        let zip = zip::ZipArchive::new(file).map_err(|e| open_err(&e))?;
        let mut names: Vec<(u32, String)> = zip
            .file_names()
            .filter_map(|n| dex_order(n).map(|k| (k, n.to_string())))
            .collect();
        names.sort();
        if names.is_empty() {
            return Err(Error::Apk(format!("no classes*.dex in {}", path.display())));
        }
        let entries = names
            .into_iter()
            .map(|(_, name)| DexEntry {
                name,
                manager: None,
                descriptors: None,
            })
            .collect();
        Ok(ApkSession {
            zip,
            entries,
            missing: HashSet::new(),
        })
    }

    fn manager(&mut self, i: usize) -> Result<&mut DexManager> {
        let entry = &mut self.entries[i];
        if entry.manager.is_none() {
            let mut buf = Vec::new();
            self.zip
                .by_name(&entry.name)
                .and_then(|mut f| f.read_to_end(&mut buf).map_err(Into::into))
                .map_err(|e| Error::Apk(format!("cannot read {}: {e}", entry.name)))?;
            entry.manager = Some(DexManager::from_bytes(buf, false));
        }
        Ok(entry.manager.as_mut().unwrap())
    }

    fn dex(&mut self, i: usize) -> Result<&DEX> {
        Ok(self
            .manager(i)?
            .dex
            .as_ref()
            .expect("DexManager::from_bytes always parses"))
    }

    /// Index of the DEX entry that defines `class`, inflating entries until one does.
    pub fn find(&mut self, class: &ClassName) -> Result<Option<usize>> {
        if self.missing.contains(class) {
            return Ok(None);
        }
        for i in 0..self.entries.len() {
            if self.dex(i)?.get_class(class.descriptor()).is_some() {
                return Ok(Some(i));
            }
        }
        self.missing.insert(class.clone());
        Ok(None)
    }

    fn descriptors(&mut self, i: usize) -> Result<&[ClassName]> {
        if self.entries[i].descriptors.is_none() {
            let dex = self.dex(i)?;
            let all = (0..dex.classes_len())
                .map(|k| ClassName::from_dex(dex.get_class_by_idx(k).fullname()))
                .collect();
            self.entries[i].descriptors = Some(all);
        }
        Ok(self.entries[i].descriptors.as_deref().unwrap())
    }

    /// Direct supertypes of `class` (superclass, then interfaces) that this APK defines.
    fn in_apk_supertypes(&mut self, class: &ClassName) -> Result<Vec<ClassName>> {
        let Some(i) = self.find(class)? else {
            return Ok(Vec::new());
        };
        let dex = self.dex(i)?;
        let mut def = dex.get_class(class.descriptor()).expect("find succeeded");
        let mut type_ids = vec![def.superclass_idx()];
        type_ids.extend(def.interface_type_ids().iter().map(|&t| u32::from(t)));
        let named: Vec<ClassName> = type_ids
            .into_iter()
            .filter(|&t| t != NO_INDEX)
            .map(|t| ClassName::from_dex(dex.get_type(t).descriptor))
            .collect();
        let mut out = Vec::new();
        for sup in named {
            if self.find(&sup)?.is_some() {
                out.push(sup);
            }
        }
        Ok(out)
    }

    /// The classes JADX needs to print `target` as a full-APK run would: the target, its inner
    /// classes (same DEX), then its in-APK superclasses and interfaces, transitively.
    pub fn context(&mut self, target: &ClassName) -> Result<Vec<ClassName>> {
        let i = self
            .find(target)?
            .ok_or_else(|| Error::ClassNotFound(target.dotted()))?;
        let mut out = vec![target.clone()];
        out.extend(
            self.descriptors(i)?
                .iter()
                .filter(|d| d.is_inner_of(target))
                .cloned(),
        );
        let mut seen: HashSet<ClassName> = out.iter().cloned().collect();
        let mut queue = vec![target.clone()];
        while let Some(class) = queue.pop() {
            for sup in self.in_apk_supertypes(&class)? {
                if seen.insert(sup.clone()) {
                    out.push(sup.clone());
                    queue.push(sup);
                }
            }
        }
        Ok(out)
    }

    pub fn extract(&mut self, class: &ClassName) -> Result<MiniDex> {
        let i = self
            .find(class)?
            .ok_or_else(|| Error::ClassNotFound(class.dotted()))?;
        let bytes = fix_header(self.manager(i)?.extract_and_rebuild(class.descriptor()));
        Ok(MiniDex {
            class: class.clone(),
            bytes,
        })
    }

    /// Mini DEX per class of `classes` (from `context`), plus the in-APK owners of every symbol
    /// that the printed classes (target and its inner classes) name with something that is not
    /// a Java identifier. JADX renames R8 `-CC`, `$r8$lambda$...`, Kotlin `foo-0680j_4` and
    /// `X$1` only for classes it was given, so those owners must be inputs too.
    pub fn extract_with_owners(
        &mut self,
        target: &ClassName,
        classes: &[ClassName],
    ) -> Result<Vec<MiniDex>> {
        let mut blobs = Vec::with_capacity(classes.len());
        for class in classes {
            blobs.push(self.extract(class)?);
        }
        let mut owners = BTreeSet::new();
        for blob in blobs
            .iter()
            .filter(|b| b.class == *target || b.class.is_inner_of(target))
        {
            owners.extend(invalid_name_owners(&blob.bytes));
        }
        let have: HashSet<&ClassName> = classes.iter().collect();
        for owner in owners {
            if !have.contains(&owner) && self.find(&owner)?.is_some() {
                blobs.push(self.extract(&owner)?);
            }
        }
        Ok(blobs)
    }
}

/// `classes.dex` is 1, `classesN.dex` is N; anything else is not a DEX entry.
fn dex_order(name: &str) -> Option<u32> {
    let digits = name.strip_prefix("classes")?.strip_suffix(".dex")?;
    if digits.is_empty() {
        Some(1)
    } else if digits.bytes().all(|b| b.is_ascii_digit()) {
        digits.parse().ok()
    } else {
        None
    }
}

/// rust_ASC's rebuilt DEX has a zeroed checksum and signature, which JADX rejects.
fn fix_header(mut b: Vec<u8>) -> Vec<u8> {
    let sig = Sha1::digest(&b[32..]);
    b[12..32].copy_from_slice(&sig);
    let sum = adler32(&b[12..]);
    b[8..12].copy_from_slice(&sum.to_le_bytes());
    b
}

fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

fn is_java_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$' => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        }
        _ => false,
    }
}

/// Classes that `mini_dex` names, or whose members it names, with something that is not a
/// valid Java identifier.
fn invalid_name_owners(mini_dex: &[u8]) -> BTreeSet<ClassName> {
    let dex = DEX::parse(mini_dex, "mini");
    let mut owners = BTreeSet::new();
    for k in 0..dex.header.types.1 {
        let descriptor = dex.get_type(k).descriptor;
        let d = descriptor.trim_start_matches('[');
        if !d.starts_with('L') {
            continue;
        }
        let segments = d[1..d.len() - 1]
            .split(['/', '$'])
            .filter(|s| !s.is_empty());
        if !segments.clone().all(is_java_name) {
            owners.insert(ClassName::from_dex(d.to_string()));
        }
    }
    for k in 0..dex.header.methods.1 {
        let m = dex.get_method(k);
        let name = m.name();
        if name != "<init>" && name != "<clinit>" && !is_java_name(&name) {
            owners.insert(ClassName::from_dex(m.cls().fullname));
        }
    }
    for k in 0..dex.header.fields.1 {
        let f = dex.get_field(k);
        if !is_java_name(&f.name()) {
            owners.insert(ClassName::from_dex(f.cls().fullname));
        }
    }
    owners
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adler32_matches_the_zlib_reference_values() {
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn dex_entry_order_puts_classes_dex_first() {
        assert_eq!(dex_order("classes.dex"), Some(1));
        assert_eq!(dex_order("classes2.dex"), Some(2));
        assert_eq!(dex_order("classes12.dex"), Some(12));
        assert_eq!(dex_order("classesX.dex"), None);
        assert_eq!(dex_order("lib/classes.dex"), None);
    }

    #[test]
    fn java_identifier_rule_is_ascii_only() {
        assert!(is_java_name("Foo$Bar"));
        assert!(is_java_name("_x1"));
        assert!(!is_java_name("1"));
        assert!(!is_java_name("roundToPx-0680j_4"));
        assert!(!is_java_name("-CC"));
        assert!(!is_java_name(""));
    }
}
