use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    BadClassName(String),
    ClassNotFound(String),
    Apk(String),
    Jadx(String),
    BadRequest(String),
    Internal(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadClassName(s) => write!(f, "not a valid class name: {s:?}"),
            Error::ClassNotFound(s) => write!(f, "class not found in APK: {s}"),
            Error::Apk(m) | Error::BadRequest(m) | Error::Internal(m) => f.write_str(m),
            Error::Jadx(m) => write!(f, "JADX failed: {m}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
