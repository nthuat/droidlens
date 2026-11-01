//! Text passes over JADX's Java output: drop lines an agent never needs, and cut grep-style
//! snippets around matches.

/// Removes Kotlin metadata annotations and JADX's provenance comments. Each is one line in
/// JADX output, so dropping whole lines keeps the Java valid.
pub fn strip_noise(code: &str) -> String {
    let noise = |line: &str| {
        let t = line.trim_start();
        t.starts_with("@Metadata(")
            || t.starts_with("@DebugMetadata(")
            || t.starts_with("/* JADX INFO: loaded from:")
            || t.starts_with("/* JADX INFO: compiled from:")
    };
    let mut out: String = code
        .lines()
        .filter(|l| !noise(l))
        .collect::<Vec<_>>()
        .join("\n");
    if code.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Lines containing `needle`, each with `context` lines around it, numbered from 1 like grep -n.
/// Overlapping windows merge; separate windows are split by `--`. Stops after `max_lines`
/// printed lines. `None` when nothing matches.
pub fn snippets(code: &str, needle: &str, context: usize, max_lines: usize) -> Option<String> {
    let lines: Vec<&str> = code.lines().collect();
    let hits: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.contains(needle))
        .map(|(i, _)| i)
        .collect();
    if needle.is_empty() || hits.is_empty() {
        return None;
    }
    let mut windows: Vec<(usize, usize)> = Vec::new();
    for h in hits {
        let (lo, hi) = (
            h.saturating_sub(context),
            (h + context).min(lines.len() - 1),
        );
        match windows.last_mut() {
            Some(w) if lo <= w.1 + 1 => w.1 = hi,
            _ => windows.push((lo, hi)),
        }
    }
    let mut out = Vec::new();
    let mut printed = 0;
    for (n, (lo, hi)) in windows.into_iter().enumerate() {
        if n > 0 {
            out.push("--".to_string());
        }
        for (i, line) in lines.iter().enumerate().take(hi + 1).skip(lo) {
            if printed == max_lines {
                out.push("[more matches in this class; call getclass for all of it]".into());
                return Some(out.join("\n"));
            }
            out.push(format!("{:>5}: {line}", i + 1));
            printed += 1;
        }
    }
    Some(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_noise_drops_metadata_and_provenance_lines_only() {
        let code = "package a;\n\n/* JADX INFO: compiled from: r8-map */\n/* JADX INFO: loaded from: /tmp/x/000.dex */\n@Metadata(d1 = {\"x\"}, k = 1)\npublic class A {\n    /* JADX INFO: renamed from: b */\n    @DebugMetadata(f = \"A.kt\")\n    void b() {}\n}\n";
        assert_eq!(
            strip_noise(code),
            "package a;\n\npublic class A {\n    /* JADX INFO: renamed from: b */\n    void b() {}\n}\n"
        );
    }

    #[test]
    fn snippets_merge_close_matches_and_number_lines() {
        let code = (1..=20)
            .map(|i| {
                if i == 5 || i == 7 || i == 15 {
                    format!("hit {i}")
                } else {
                    format!("line {i}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let out = snippets(&code, "hit", 1, 100).unwrap();
        assert_eq!(
            out,
            "    4: line 4\n    5: hit 5\n    6: line 6\n    7: hit 7\n    8: line 8\n--\n   14: line 14\n   15: hit 15\n   16: line 16"
        );
        assert_eq!(snippets(&code, "absent", 1, 100), None);
    }

    #[test]
    fn snippets_stop_at_max_lines() {
        let code = "hit\nhit\nhit\nhit";
        let out = snippets(code, "hit", 0, 2).unwrap();
        assert_eq!(
            out,
            "    1: hit\n    2: hit\n[more matches in this class; call getclass for all of it]"
        );
    }
}
