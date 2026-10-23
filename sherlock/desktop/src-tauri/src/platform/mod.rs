pub mod clipboard;
pub mod fs;
pub mod gpu;
pub mod paths;
pub mod process;
pub mod python;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
// Variants are constructed via cfg-conditional blocks in current_os(); clippy sees only one on each target.
#[allow(dead_code)]
pub enum OsKind {
    Linux,
    MacOS,
    Windows,
}

/// Platform-specific PDFium shared library filename.
pub fn pdfium_lib_name() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "libpdfium.so"
    }
    #[cfg(target_os = "macos")]
    {
        "libpdfium.dylib"
    }
    #[cfg(target_os = "windows")]
    {
        "pdfium.dll"
    }
}

pub fn current_os() -> OsKind {
    #[cfg(target_os = "linux")]
    {
        OsKind::Linux
    }
    #[cfg(target_os = "macos")]
    {
        OsKind::MacOS
    }
    #[cfg(target_os = "windows")]
    {
        OsKind::Windows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_os_returns_valid_variant() {
        let os = current_os();
        // Just verify it returns one of the valid variants without panicking
        match os {
            OsKind::Linux | OsKind::MacOS | OsKind::Windows => {}
        }
    }

    #[test]
    fn os_kind_is_copy_and_eq() {
        let a = current_os();
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn pdfium_lib_name_has_correct_extension() {
        let name = pdfium_lib_name();
        assert!(
            name.ends_with(".so") || name.ends_with(".dylib") || name.ends_with(".dll"),
            "unexpected lib name: {name}"
        );
    }
}
