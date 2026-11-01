// Implement static dispatch to allow `getfacl` and `setfacl` to accept
// arguments that represent file system paths or open file descriptors.
//
// This code also addresses an ergonomic issue to permit passing a single path
// to `setfacl` without wrapping it in `&[]`. This only works for std library
// classes that implement AsRef<Path>; not third-party classes.
//
// Here are the compromises:
//
// - This code is designed to be call-site compatible with the prior API which
//   accepted only file system paths (AsRef<Path>).
// - There's no handling for `std::fs::File` because it must be passed by
//   reference and that coherence-conflicts with AsRef<Path>. You must
//   call `file.as_fd()`.
// - The ergonomic fix specifies a fixed list of classes from the standard
//   library instead of AsRef<Path> due to issues with coherence.

use crate::acl::{Acl, AclOption};
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::path::{Path, PathBuf};

/// `AclFile` represents a file system path or an open file descriptor:
///
/// - Any type that implements `AsRef<Path>`
/// - `std::os::fd::BorrowedFd`
///
#[derive(Debug)]
pub enum AclFile<'a> {
    Path(&'a Path),
    Fd(BorrowedFd<'a>),
}

/// `AclFilePaths` represents a slice of file system paths or a single open
/// file descriptor. It also accepts a single file system path (from std).
///
/// - `std::os::fd::BorrowedFd`
/// - `&Path`, `&PathBuf`, `&str`, `&String`, `&OsStr`, `&OsString`
/// - A slice or fixed array of `AsRef<Path>`
///
/// Note that the elements of the slice can be third-party classes that
/// implement `AsRef<Path>` but the single argument must be one of the
/// specified classes only.
pub trait AclFilePaths<'a> {
    type Iter: Iterator<Item = AclFile<'a>>;
    fn file_iter(self) -> Self::Iter;
}

// AclFile Implementations

impl<'a, P> From<&'a P> for AclFile<'a>
where
    P: AsRef<Path> + ?Sized,
{
    fn from(path: &'a P) -> Self {
        AclFile::Path(path.as_ref())
    }
}

impl<'a> From<BorrowedFd<'a>> for AclFile<'a> {
    fn from(fd: BorrowedFd<'a>) -> Self {
        AclFile::Fd(fd)
    }
}

// AclFilePaths Implementations

impl<'a> AclFilePaths<'a> for &'a Path {
    type Iter = std::iter::Once<AclFile<'a>>;
    fn file_iter(self) -> Self::Iter {
        std::iter::once(AclFile::from(self))
    }
}

impl<'a> AclFilePaths<'a> for &'a PathBuf {
    type Iter = std::iter::Once<AclFile<'a>>;
    fn file_iter(self) -> Self::Iter {
        std::iter::once(AclFile::from(self))
    }
}

impl<'a> AclFilePaths<'a> for &'a str {
    type Iter = std::iter::Once<AclFile<'a>>;
    fn file_iter(self) -> Self::Iter {
        std::iter::once(AclFile::from(self))
    }
}

impl<'a> AclFilePaths<'a> for &'a String {
    type Iter = std::iter::Once<AclFile<'a>>;
    fn file_iter(self) -> Self::Iter {
        std::iter::once(AclFile::from(self))
    }
}

impl<'a> AclFilePaths<'a> for &'a OsStr {
    type Iter = std::iter::Once<AclFile<'a>>;
    fn file_iter(self) -> Self::Iter {
        std::iter::once(AclFile::from(self))
    }
}

impl<'a> AclFilePaths<'a> for &'a OsString {
    type Iter = std::iter::Once<AclFile<'a>>;
    fn file_iter(self) -> Self::Iter {
        std::iter::once(AclFile::from(self))
    }
}

impl<'a> AclFilePaths<'a> for BorrowedFd<'a> {
    type Iter = std::iter::Once<AclFile<'a>>;
    fn file_iter(self) -> Self::Iter {
        std::iter::once(AclFile::from(self))
    }
}

impl<'a, T> AclFilePaths<'a> for &'a [T]
where
    T: AsRef<Path>,
{
    type Iter = std::iter::Map<std::slice::Iter<'a, T>, fn(&'a T) -> AclFile<'a>>;

    fn file_iter(self) -> Self::Iter {
        self.iter().map(AclFile::from)
    }
}

impl<'a, T, const N: usize> AclFilePaths<'a> for &'a [T; N]
where
    T: AsRef<Path>,
{
    type Iter = std::iter::Map<std::slice::Iter<'a, T>, fn(&'a T) -> AclFile<'a>>;

    fn file_iter(self) -> Self::Iter {
        self.as_slice().iter().map(AclFile::from)
    }
}

impl AclFile<'_> {
    /// Read native ACL object from the file (either path or fd).
    pub fn read(&self, options: AclOption) -> io::Result<Acl> {
        match self {
            AclFile::Path(path) => Acl::read(path, options),
            AclFile::Fd(fd) => Acl::read_fd(fd.as_raw_fd(), options),
        }
    }

    /// Write native ACL object to the file (either path or fd).
    pub fn write(&self, acl: &Acl, options: AclOption) -> io::Result<()> {
        match self {
            AclFile::Path(path) => acl.write(path, options),
            AclFile::Fd(fd) => acl.write_fd(fd.as_raw_fd(), options),
        }
    }
}

// =============================== T E S T S ================================ //

#[cfg(test)]
mod tests {
    use super::{AclFile, AclFilePaths};
    use std::os::fd::AsFd;
    use std::path::{Path, PathBuf};
    use tempfile::NamedTempFile;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    // Demo call site for Into<AclFile>.
    fn check_1_arg<'a, T: Into<AclFile<'a>>>(file: T) {
        println!("{:?}", file.into());
    }

    // Demo call site for AclFilePaths.
    fn check_n_args<'a, T: AclFilePaths<'a>>(files: T) {
        for file in files.file_iter() {
            print!("{file:?}, ");
        }
        println!();
    }
    #[test]
    fn test_aclfile() -> TestResult {
        // NamedTempFile is a third-party class that implements AsRef<Path>.
        let file1 = NamedTempFile::new()?;

        check_1_arg("x");
        check_1_arg(Path::new("y"));
        check_1_arg(&file1);
        check_1_arg(file1.as_fd());

        Ok(())
    }

    #[test]
    fn test_aclfilepaths() -> TestResult {
        // NamedTempFile is a third-party class that implements AsRef<Path>.
        let file1 = NamedTempFile::new()?;

        check_n_args("x");
        check_n_args(&String::from("y"));
        check_n_args(Path::new("z"));
        check_n_args(&PathBuf::new());
        check_n_args(file1.as_ref()); // must call as_ref()
        check_n_args(file1.as_fd()); // must call as_fd()

        check_n_args(&["x", "y"]);
        check_n_args(&[Path::new("x"), Path::new("y")]);
        check_n_args(&[&file1]); // as_ref() not required
        check_n_args(&[file1]); // as_ref() not required [moved]

        Ok(())
    }
}
