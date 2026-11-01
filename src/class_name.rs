use crate::error::{Error, Result};

/// A Dalvik class descriptor such as `Lpkg/Outer$Inner;`.
///
/// Built by `parse` at the program's boundaries (CLI arguments, JSON and MCP inputs) or by
/// `from_dex` for descriptors read out of a DEX type table, so every value is well formed.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ClassName(String);

impl ClassName {
    /// Accepts `pkg.Outer$Inner` or `Lpkg/Outer$Inner;`.
    pub fn parse(s: &str) -> Result<ClassName> {
        let s = s.trim();
        let desc = if s.starts_with('L') && s.ends_with(';') && !s.contains('.') {
            s.to_string()
        } else {
            format!("L{};", s.replace('.', "/"))
        };
        let body = &desc[1..desc.len() - 1];
        let segment_ok = |seg: &str| !seg.is_empty() && !seg.chars().any(|c| c == ';' || c.is_whitespace());
        if body.is_empty() || !body.split('/').all(segment_ok) {
            return Err(Error::BadClassName(s.to_string()));
        }
        Ok(ClassName(desc))
    }

    pub(crate) fn from_dex(descriptor: String) -> ClassName {
        ClassName(descriptor)
    }

    pub fn descriptor(&self) -> &str {
        &self.0
    }

    /// `pkg.Outer$Inner`, the form JADX and humans use.
    pub fn dotted(&self) -> String {
        self.0[1..self.0.len() - 1].replace('/', ".")
    }

    /// Every inner class descriptor of this class starts with `Lpkg/Outer$`.
    pub fn inner_prefix(&self) -> String {
        format!("{}$", &self.0[..self.0.len() - 1])
    }

    pub fn is_inner_of(&self, outer: &ClassName) -> bool {
        self.0.starts_with(&outer.inner_prefix())
    }
}

impl std::fmt::Display for ClassName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotted_and_descriptor_forms_parse_to_the_same_name() {
        let a = ClassName::parse("org.example.Outer$Inner").unwrap();
        let b = ClassName::parse("Lorg/example/Outer$Inner;").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.descriptor(), "Lorg/example/Outer$Inner;");
        assert_eq!(a.dotted(), "org.example.Outer$Inner");
        assert_eq!(a.inner_prefix(), "Lorg/example/Outer$Inner$");
    }

    #[test]
    fn malformed_names_are_rejected() {
        for bad in ["", "a//b", "a b", "La/b;;", "Lfoo.Bar;"] {
            assert!(ClassName::parse(bad).is_err(), "{bad:?} should be rejected");
        }
    }
}
