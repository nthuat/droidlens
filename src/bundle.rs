//! Split-APK bundles (XAPK, APKS, APKM, or a folder of split APKs) become one merged APK, so
//! every tool reads them like a plain APK. The merged file holds the base APK's manifest and
//! resources plus the DEX files of every split, renumbered base first, and is cached under
//! `$DROIDLENS_HOME/bundles` keyed by the input's path, size and modification time.

use std::fs::{self, File};
use std::io::{Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::error::{Error, Result};
use crate::jadx::{atomic, home};

/// One APK inside a bundle.
struct Split {
    name: String,
    bytes: Vec<u8>,
}

/// The path every tool should read: `path` itself for a plain APK, else the cached merged APK.
pub fn resolve(path: &Path) -> Result<PathBuf> {
    let splits = match bundle_splits(path)? {
        None => return Ok(path.to_path_buf()),
        Some(s) => s,
    };
    let dest = home()?
        .join("bundles")
        .join(format!("{}.apk", cache_key(path)?));
    atomic(&dest, |tmp| merge(path, splits, tmp))?;
    Ok(dest)
}

/// `None` for a plain APK (or anything that is not a bundle, so the caller reports its own error).
fn bundle_splits(path: &Path) -> Result<Option<Vec<Split>>> {
    let err =
        |e: &dyn std::fmt::Display| Error::Apk(format!("cannot open {}: {e}", path.display()));
    if path.is_dir() {
        let mut splits = Vec::new();
        for entry in fs::read_dir(path).map_err(|e| err(&e))? {
            let p = entry.map_err(|e| err(&e))?.path();
            if p.extension().is_some_and(|x| x == "apk") {
                let name = p.file_name().unwrap().to_string_lossy().into_owned();
                splits.push(Split {
                    name,
                    bytes: fs::read(&p).map_err(|e| err(&e))?,
                });
            }
        }
        return Ok((!splits.is_empty()).then_some(splits));
    }
    let Ok(file) = File::open(path) else {
        return Ok(None);
    };
    let Ok(mut zip) = ZipArchive::new(file) else {
        return Ok(None);
    };
    let is_apk = zip
        .file_names()
        .any(|n| n == "AndroidManifest.xml" || n == "classes.dex");
    if is_apk {
        return Ok(None);
    }
    let names: Vec<String> = zip
        .file_names()
        .filter(|n| n.ends_with(".apk"))
        .map(String::from)
        .collect();
    if names.is_empty() {
        return Ok(None);
    }
    let mut splits = Vec::with_capacity(names.len());
    for name in names {
        let mut bytes = Vec::new();
        zip.by_name(&name)
            .map_err(|e| err(&e))?
            .read_to_end(&mut bytes)
            .map_err(|e| err(&e))?;
        splits.push(Split { name, bytes });
    }
    Ok(Some(splits))
}

fn cache_key(path: &Path) -> Result<String> {
    let canon = fs::canonicalize(path)
        .map_err(|e| Error::Apk(format!("cannot open {}: {e}", path.display())))?;
    let meta = fs::metadata(&canon)
        .map_err(|e| Error::Apk(format!("cannot open {}: {e}", path.display())))?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    let mut h = Sha256::new();
    h.update(canon.as_os_str().as_encoded_bytes());
    h.update(meta.len().to_le_bytes());
    h.update(mtime.to_le_bytes());
    let hex: String = h
        .finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect();
    let stem = canon
        .file_stem()
        .map_or_else(|| "bundle".into(), |s| s.to_string_lossy().into_owned());
    Ok(format!("{stem}-{hex}"))
}

/// `classes.dex` is 1, `classesN.dex` is N; other names are not DEX entries.
fn dex_index(name: &str) -> Option<u32> {
    let n = name.strip_prefix("classes")?.strip_suffix(".dex")?;
    if n.is_empty() {
        Some(1)
    } else {
        n.parse().ok().filter(|&k| k >= 2)
    }
}

/// Configuration splits (languages, densities, ABIs) carry no code; the base is the split with
/// the most DEX files, preferring the conventional base names.
fn base_rank(name: &str, dex_count: usize) -> (bool, usize) {
    let file = name.rsplit('/').next().unwrap_or(name);
    let conventional = matches!(file, "base.apk" | "base-master.apk");
    (conventional, dex_count)
}

fn merge(source: &Path, splits: Vec<Split>, dest: &Path) -> Result<()> {
    let bad = |e: &dyn std::fmt::Display| {
        Error::Apk(format!("cannot read bundle {}: {e}", source.display()))
    };
    let mut opened = Vec::new();
    for s in splits {
        let Ok(zip) = ZipArchive::new(Cursor::new(s.bytes)) else {
            continue;
        };
        let mut dex: Vec<(u32, String)> = zip
            .file_names()
            .filter_map(|n| dex_index(n).map(|k| (k, n.to_string())))
            .collect();
        dex.sort();
        opened.push((s.name, zip, dex));
    }
    let base = opened
        .iter()
        .enumerate()
        .max_by_key(|(_, (name, _, dex))| base_rank(name, dex.len()))
        .map(|(i, _)| i)
        .filter(|&i| !opened[i].2.is_empty())
        .ok_or_else(|| {
            Error::Apk(format!(
                "no split with classes*.dex in {}",
                source.display()
            ))
        })?;
    let base_split = opened.remove(base);
    opened.sort_by(|a, b| a.0.cmp(&b.0));
    opened.insert(0, base_split);

    let out = File::create(dest)
        .map_err(|e| Error::Internal(format!("cannot write {}: {e}", dest.display())))?;
    let mut w = ZipWriter::new(out);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut next = 1u32;
    for (i, (_, zip, dex)) in opened.iter_mut().enumerate() {
        if i == 0 {
            for name in ["AndroidManifest.xml", "resources.arsc"] {
                if zip.file_names().any(|n| n == name) {
                    copy(zip, name, name, &mut w, stored).map_err(|e| bad(&e))?;
                }
            }
        }
        for (_, name) in dex.iter() {
            let target = if next == 1 {
                "classes.dex".to_string()
            } else {
                format!("classes{next}.dex")
            };
            copy(zip, name, &target, &mut w, stored).map_err(|e| bad(&e))?;
            next += 1;
        }
    }
    w.finish().map_err(|e| bad(&e))?;
    Ok(())
}

fn copy<R: Read + Seek, W: Write + Seek>(
    zip: &mut ZipArchive<R>,
    from: &str,
    to: &str,
    w: &mut ZipWriter<W>,
    opts: SimpleFileOptions,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    zip.by_name(from)?.read_to_end(&mut bytes)?;
    w.start_file(to, opts)?;
    w.write_all(&bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apk(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            w.start_file(*name, SimpleFileOptions::default()).unwrap();
            w.write_all(bytes).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn merge_puts_base_dex_first_and_renumbers_feature_split_dex() {
        let dir = tempfile::tempdir().unwrap();
        let splits = vec![
            Split {
                name: "config.en.apk".into(),
                bytes: apk(&[("AndroidManifest.xml", b"cfg")]),
            },
            Split {
                name: "split_feature.apk".into(),
                bytes: apk(&[("AndroidManifest.xml", b"f"), ("classes.dex", b"F1")]),
            },
            Split {
                name: "com.example.apk".into(),
                bytes: apk(&[
                    ("AndroidManifest.xml", b"base"),
                    ("resources.arsc", b"res"),
                    ("classes.dex", b"B1"),
                    ("classes2.dex", b"B2"),
                ]),
            },
        ];
        let dest = dir.path().join("merged.apk");
        merge(Path::new("x.xapk"), splits, &dest).unwrap();
        let mut z = ZipArchive::new(File::open(&dest).unwrap()).unwrap();
        let mut read = |n: &str| {
            let mut s = String::new();
            z.by_name(n).unwrap().read_to_string(&mut s).unwrap();
            s
        };
        assert_eq!(read("AndroidManifest.xml"), "base");
        assert_eq!(read("resources.arsc"), "res");
        assert_eq!(read("classes.dex"), "B1");
        assert_eq!(read("classes2.dex"), "B2");
        assert_eq!(read("classes3.dex"), "F1");
    }

    #[test]
    fn plain_apk_and_non_zip_are_not_bundles() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("a.apk");
        fs::write(
            &plain,
            apk(&[("AndroidManifest.xml", b"m"), ("classes.dex", b"d")]),
        )
        .unwrap();
        assert!(bundle_splits(&plain).unwrap().is_none());
        let text = dir.path().join("hosts");
        fs::write(&text, b"not a zip").unwrap();
        assert!(bundle_splits(&text).unwrap().is_none());
    }

    #[test]
    fn xapk_lists_its_inner_apks() {
        let dir = tempfile::tempdir().unwrap();
        let x = dir.path().join("a.xapk");
        let inner = apk(&[("classes.dex", b"d")]);
        fs::write(
            &x,
            apk(&[
                ("manifest.json", b"{}"),
                ("com.a.apk", &inner),
                ("config.en.apk", &inner),
            ]),
        )
        .unwrap();
        let names: Vec<String> = bundle_splits(&x)
            .unwrap()
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, ["com.a.apk", "config.en.apk"]);
    }
}
